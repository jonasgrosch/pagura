"""User-authorized continuation of a fixed interrupted schedule, retaining its prefix."""
import argparse
import copy
import json
import os
from pathlib import Path
import platform
import random
import signal
import sys
import time

sys.dont_write_bytecode = True
import f32_painted_interval as original
from f32_interval_admission import ROOT, RESULTS, descriptor, pin, require, verify_config
from paired_render import MAX_BYTES, MAX_RETAINED, expected_image, sha, source_identity
from benchkit.artifacts import hash_regular_file, read_bounded_regular
from benchkit.images import normalize
from benchkit.measurement import MeasurementCancelled, run_process
from benchkit.statistics import ratio_bounds

NAME = 'f32-painted-interval-continuation'
RUN = 'confirmation-001'
LOCAL = RESULTS / 'local' / NAME
DRIVER = 'benchmarks/f32_painted_interval_continuation.py'
FREEZER = 'benchmarks/prepare_f32_painted_interval_continuation.py'
AUDITOR = 'benchmarks/evaluate_f32_painted_interval_continuation.py'
DOC = 'docs/plans/f32-painted-interval-continuation.md'
ORIGINAL_LOCAL = RESULTS / 'local/f32-painted-interval'
SIDES = ('baseline', 'candidate')


def load(path):
    return json.loads(read_bounded_regular(path, MAX_BYTES))


def schedule():
    rng = random.Random(20261110)
    cells = list(original.CELLS)
    rng.shuffle(cells)
    result = []
    for index, cell in enumerate(cells):
        for pair in range(-5, 200):
            order = list(SIDES)
            rng.shuffle(order)
            result.append({'sequence': len(result), 'cell_index': index, 'cell': cell, 'pair': pair, 'order': order})
    return cells, result


def make_plan(old, amendment):
    plan = copy.deepcopy(old)
    plan.update(experiment=NAME, status='frozen-before-continuation', amendment=amendment,
                maximum_new_segments=1, automatic_retry=False,
                scope='Post-stop method amendment retaining the original complete prefix and finishing the same schedule; not an uninterrupted or fresh confirmation.')
    return plan


def original_evidence(amendment):
    require(amendment['schema'] == 1 and amendment['experiment'] == NAME
            and amendment['status'] == 'reviewed-user-authorized-continuation'
            and amendment['decision'] == 'retain-every-completed-pair-and-resume-unfinished-fixed-schedule'
            and amendment['reason'].strip() and amendment['process_presence_is_not_cpu_measurement'] is True,
            'Explicit user-authorized method amendment required')
    old_config = amendment['original_config']
    require(old_config['path'] == str(ORIGINAL_LOCAL / 'confirmation-001-config.json'), 'Wrong original config')
    config, bindings = verify_config(Path(old_config['path']), old_config['sha256'], 'confirmation')
    items = amendment['original']
    expected = {'receipt': 'confirmation-001-receipt.json', 'guard': 'confirmation-001-observer/guard.json',
                'report': 'confirmation-001/report.json', 'intent': 'confirmation-001-launch.json',
                'start': 'confirmation-001-driver-start.json'}
    require(set(items) == set(expected) and all(items[k]['path'] == str(ORIGINAL_LOCAL / v) for k, v in expected.items()), 'Wrong original segment paths')
    require(all(descriptor(item['path']) == item for item in items.values()), 'Original evidence changed')
    # The interrupted numerical report is deliberately not parsed here.
    receipt, guard = pin(items['receipt']), pin(items['guard'])
    require(receipt['status'] == 'failed' and all(type(receipt.get(k)) is int and receipt[k] == 143 for k in ('exit_code', 'wrapper_exit_code'))
            and receipt['source_unchanged'] is True and not receipt['errors'] and not receipt['artifact_errors']
            and receipt['source_root'] == str(ROOT)
            and receipt['source_before'] == receipt['source_after'] == bindings['runner_sources']['candidate']
            and guard['status'] == 'invalid-external-contention' and any(e['matches'] for e in guard['events']),
            'Expected original process-presence stop is not established')
    launcher = [bindings['python']['path'], str(ROOT / 'benchmarks/launch_f32_painted_interval.py'),
                '--config', old_config['path'], '--config-sha256', old_config['sha256'], '--phase', 'confirmation']
    require(guard['command'] == launcher and receipt['argv'] == [bindings['python']['path'], bindings['guard']['path'],
            '--evidence-dir', str(Path(items['guard']['path']).parent), '--', *launcher], 'Original launch chain differs')
    for item in (old_config, *(items[k] for k in ('guard', 'report', 'intent', 'start'))):
        require(receipt['artifacts'].get(item['path']) == item['sha256'], 'Original final receipt misses evidence')
    return config, bindings


def validate(config_path, digest):
    require(config_path == LOCAL / 'confirmation-config.json' and Path.cwd().resolve() == ROOT, 'Fixed continuation config/cwd required')
    config = pin({'path': str(config_path), 'sha256': digest})
    require(config['schema'] == 1 and config['experiment'] == NAME and config['status'] == 'frozen-before-launch', 'Invalid continuation config')
    amendment = pin(config['amendment'])
    old_config, old = original_evidence(amendment)
    bindings, plan = pin(config['bindings']), pin(config['plan'])
    require(config['plan']['path'] == str(RESULTS / (NAME + '-plan.json'))
            and config['bindings']['path'] == str(RESULTS / (NAME + '-bindings.json'))
            and plan == make_plan(pin(old['plan']), config['amendment']), 'Continuation plan differs')
    expected = copy.deepcopy(old)
    expected.update(experiment=NAME, plan=config['plan'], amendment=config['amendment'],
                    original_config=amendment['original_config'])
    new_names = {DRIVER, FREEZER, DOC, 'benchmarks/results/' + NAME + '-plan.json'}
    require(not (new_names & old['bound_files_sha256'].keys()), 'New names overwrite original helper closure')
    expected['bound_files_sha256'].update({n: hash_regular_file(ROOT / n) for n in new_names})
    require(bindings == expected and config['original_config'] == amendment['original_config'], 'Original core/helper/evidence binding changed')
    for key in ('python', 'python_executable', 'guard'):
        require(config[key] == old_config[key], 'Runtime/observer changed')
    return config, bindings, amendment


def commands(config, path, digest):
    prefix = [config['python']['path'], str(ROOT / DRIVER), '--config', str(path), '--config-sha256', digest]
    return prefix, [*prefix, '--execute'], [config['python']['path'], config['guard']['path'], '--evidence-dir',
                                          str(LOCAL / (RUN + '-observer')), '--', *prefix]


def complete_pair(pair):
    """Structural completion only; never compare timings or choose fast samples."""
    fields = {'command', 'process', 'diagnostic', 'stages', 'blob', 'output_sha256', 'pixels_sha256', 'width', 'height', 'oracle_equal'}
    return all(isinstance(pair.get(s), dict) and fields <= pair[s].keys() for s in SIDES)


def prefix(report, cells, ordered):
    require(report['experiment'] == 'f32-painted-interval' and report['phase'] == 'confirmation'
            and (report['pairs'], report['warmups'], report['seed']) == (200, 5, 20261110), 'Original schedule differs')
    kept, abandoned, ended = [], [], False
    require(len(report['cells']) <= len(cells), 'Original cell count exceeds fixed schedule')
    for index, row in enumerate(report['cells']):
        require((row['case'], row['mode'], row['operation']) == cells[index] and len(row['pairs']) <= 205, 'Original cell prefix differs')
        for offset, pair in enumerate(row['pairs']):
            expected = ordered[index * 205 + offset]
            require(not ended and pair['pair'] == expected['pair'] and pair['order'] == expected['order'], 'Original pair prefix has a gap/order change')
            if complete_pair(pair):
                require(all(pair[s]['oracle_equal'] is True and type(pair[s]['process'].get('exit_code')) is int
                            and pair[s]['process']['exit_code'] == 0 and pair[s]['process'].get('status') != 'timeout' for s in SIDES)
                        and pair['baseline']['output_sha256'] == pair['candidate']['output_sha256'], 'Completed original pair has a correctness/process failure')
                kept.append((expected, copy.deepcopy(pair)))
            else:
                require(index == len(report['cells']) - 1 and offset == len(row['pairs']) - 1, 'Only the terminal pair may be incomplete')
                abandoned.append({'sequence': expected['sequence'], 'cell_index': index, 'pair': pair['pair'],
                                  'segment': 'original', 'attempt': copy.deepcopy(pair), 'reason': 'Pair structurally incomplete; retain attempt, rerun both arms.'})
                ended = True
        require(index == len(report['cells']) - 1 or len(row['pairs']) == 205, 'Original prefix skipped an unfinished cell')
    require([e['sequence'] for e, _ in kept] == list(range(len(kept))), 'Completed pairs are not a contiguous prefix')
    return kept, abandoned


def execute(config, bindings, amendment, config_pin, intent_pin, start_pin):
    directory = LOCAL / RUN
    directory.mkdir(parents=True, exist_ok=False)
    report_path, journal_path = directory / 'report.json', directory / 'journal.jsonl'
    old = pin(amendment['original']['report'])
    require(old['identities'] == bindings['identities'], 'Original measured identities differ')
    cells, ordered = schedule()
    kept, abandoned = prefix(old, cells, ordered)
    report = copy.deepcopy(old)
    report.update(experiment=NAME, original_experiment=old['experiment'], status='running-continuation', admission=False,
        amendment=config['amendment'], config=config_pin, original_report=amendment['original']['report'],
        environment_scope='retained-original',
        continuation_environment={'platform': platform.platform(), 'machine': platform.machine(), 'python': sys.version,
                                  'run_id': os.environ['PAGURA_RUN_ID'], 'run_receipt': os.environ['PAGURA_RUN_RECEIPT']},
        continuation_driver_sha256=hash_regular_file(ROOT / DRIVER),
        segments={'original': {**amendment['original'], 'directory': str(Path(amendment['original']['report']['path']).parent)},
                  'continuation': {'directory': str(directory), 'run_id': os.environ['PAGURA_RUN_ID'],
                                   'receipt': os.environ['PAGURA_RUN_RECEIPT'], 'guard': str(LOCAL / (RUN + '-observer/guard.json')),
                                   'intent': intent_pin, 'start': start_pin}},
        cells=[{'case': n, 'mode': m, 'operation': o, 'role': 'target' if n == 'sparse64' and o == 'full' else 'protected', 'pairs': []} for n, m, o in cells],
        pair_sources=[], abandoned_attempts=abandoned, original_completed_pairs=len(kept), new_completed_pairs=0,
        source_before={s: source_identity(bindings['identities'][s]['root']) for s in SIDES},
        limitations=[*old['limitations'], 'User-authorized post-stop method amendment; this is neither uninterrupted nor wholly fresh confirmation.',
                     'This amendment supersedes the original no-reuse/exclusion wording only for completed pairs from this same interrupted confirmation. No pilot or other historical run is pooled.',
                     'Continuation uses flushed journal checkpoints rather than rewriting the growing report after each pair; this is an untimed harness/cohort difference.',
                     'Original guard recorded process presence, not CPU consumption or measured contention. All completed original pairs are retained.',
                     'Cohort/time boundary is explicit; unchanged bootstrap thresholds do not prove equivalence to uninterrupted sampling.'])
    report.pop('error', None)
    for entry, pair in kept:
        report['cells'][entry['cell_index']]['pairs'].append(pair)
        report['pair_sources'].append({k: entry[k] for k in ('sequence', 'cell_index', 'pair')} | {'segment': 'original'})
    stored = 0
    journal = journal_path.open('x')
    def event(kind, **values):
        journal.write(json.dumps({'event': kind, 'monotonic_ns': time.monotonic_ns(), **values}, allow_nan=False) + '\n')
        journal.flush()
    def retain(relative, data):
        nonlocal stored
        path = directory / relative
        if path.exists():
            require(read_bounded_regular(path, MAX_BYTES) == data, 'Retained artifact changed')
        else:
            require(stored + len(data) <= MAX_RETAINED, 'Continuation retention budget exhausted')
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open('xb') as stream:
                stream.write(data)
            path.chmod(0o400)
            stored += len(data)
        return {'path': str(path), 'sha256': sha(data), 'bytes': len(data)}
    def checkpoint():
        report['stored_bytes'] = stored
        pending = directory / 'report.pending'
        pending.write_text(json.dumps(report, indent=2, allow_nan=False) + '\n')
        pending.replace(report_path)
    try:
        event('run-start', config=config_pin, original_report=amendment['original']['report'], original_completed_pairs=len(kept))
        checkpoint()
        oracles = {}
        for name in original.CASES:
            pdf, oracle = original.fixture(name)
            require(old['inputs'][name] == {'pdf_sha256': sha(pdf), 'oracle_sha256': sha(oracle)}, 'Original input/oracle differs')
            retain('inputs/' + name + '.pdf', pdf)
            retain('oracles/' + name + '.ppm', oracle)
            oracles[name] = oracle
        policy = {'timeout_seconds': 120, 'maximum_output_bytes': MAX_BYTES, 'measure_rss': True}
        for entry in ordered[len(kept):]:
            name, mode, operation = entry['cell']
            sample = {'pair': entry['pair'], 'order': entry['order'], 'started_monotonic_ns': time.monotonic_ns()}
            report['cells'][entry['cell_index']]['pairs'].append(sample)
            event('pair-start', schedule=entry, sample=sample)
            expected = expected_image(oracles[name], operation)
            output = directory / 'pending.rgba'
            for side in entry['order']:
                identity = bindings['identities'][side]
                pdf = directory / 'inputs' / (name + '.pdf')
                require(sha(read_bounded_regular(pdf, MAX_BYTES)) == old['inputs'][name]['pdf_sha256']
                        and hash_regular_file(identity['binary']) == identity['binary_sha256'], 'Input/binary changed')
                output.unlink(missing_ok=True)
                command = [identity['binary'], str(pdf), mode, operation, str(output), '72']
                record = {'command': command}
                sample[side] = record
                try:
                    process, diagnostic = run_process(command, policy)
                    record.update(process=process, diagnostic=diagnostic)
                    require(type(process.get('exit_code')) is int and process['exit_code'] == 0 and process['status'] != 'timeout', 'Sample failed; retain attempt')
                    stages = [json.loads(line) for line in diagnostic.splitlines() if line.startswith('{')]
                    require(len(stages) == 1 and isinstance(stages[0], dict), 'Invalid stage record')
                    stage = record['stages'] = stages[0]
                    require(all(type(stage.get(k)) is int and stage[k] > 0 for k in ('open_ns', 'compile_ns', 'render_ns', 'total_ns', 'metric_ns'))
                            and (stage.get('schema'), stage.get('operation'), stage.get('accuracy'), stage.get('dpi'), stage.get('width'), stage.get('height'))
                            == (1, operation, mode, 72, *expected[:2])
                            and stage['metric_ns'] == stage['total_ns' if operation == 'cold' else 'render_ns']
                            and stage['total_ns'] >= sum(stage[k] for k in ('open_ns', 'compile_ns', 'render_ns')), 'Stage identity/timing differs')
                    raw = read_bounded_regular(output, MAX_BYTES)
                    blob = 'blobs/' + sha(raw) + '.rgba'
                    retain(blob, raw)
                    actual = normalize(raw)
                    header = f'RGBA8 {expected[0]} {expected[1]} premultiplied\n'.encode()
                    record.update(blob=blob, output_sha256=sha(raw), pixels_sha256=sha(actual[2]), width=actual[0], height=actual[1], oracle_equal=actual == expected)
                    require(raw.startswith(header) and raw[len(header) + 3::4] == b'\xff' * (expected[0] * expected[1])
                            and actual == expected, 'Independent RGBA differs')
                    output.unlink()
                except BaseException as error:
                    record['error'] = f'{type(error).__name__}: {error}'
                    if output.exists():
                        record['unfinished_output'] = retain('abandoned/' + str(entry['sequence']) + '-' + side + '.rgba', read_bounded_regular(output, MAX_BYTES))
                    raise
                finally:
                    event('arm-result', sequence=entry['sequence'], side=side, record=record)
            require(sample['baseline']['output_sha256'] == sample['candidate']['output_sha256'], 'Same-policy pair bytes differ')
            sample['finished_monotonic_ns'] = time.monotonic_ns()
            report['pair_sources'].append({k: entry[k] for k in ('sequence', 'cell_index', 'pair')} | {'segment': 'continuation'})
            report['new_completed_pairs'] += 1
            event('pair-complete', sequence=entry['sequence'], sample=sample)
            if entry['pair'] == 199:
                print(f"completed {entry['cell_index'] + 1}/56: {name}/{mode}/{operation}", flush=True)
        require(len(report['pair_sources']) == 11480, 'Combined fixed schedule incomplete')
        for side in SIDES:
            identity = bindings['identities'][side]
            require(source_identity(identity['root']) == {k: identity[k] for k in ('root', 'commit', 'source_sha256')}
                    and hash_regular_file(identity['binary']) == identity['binary_sha256'], 'Source/binary changed')
        require(all(hash_regular_file(ROOT / n) == h for n, h in bindings['bound_files_sha256'].items()), 'Frozen helper changed')
        for cell in report['cells']:
            pairs = [p for p in cell['pairs'] if p['pair'] >= 0]
            require(len(pairs) == 200, 'Combined measured count differs')
            cell['latency'] = ratio_bounds([p['candidate']['stages']['metric_ns'] for p in pairs], [p['baseline']['stages']['metric_ns'] for p in pairs])
            rss = {s: [p[s]['process']['peak_rss_bytes'] for p in pairs] for s in SIDES}
            require(all(type(v) is int and v > 0 for values in rss.values() for v in values), 'Unavailable RSS')
            cell['peak_rss_ratio'] = max(rss['candidate']) / max(rss['baseline'])
        report['status'] = 'complete-amended-exploratory'
    except BaseException as error:
        report.update(status='invalid-or-incomplete', error=f'{type(error).__name__}: {error}')
        raise
    finally:
        try:
            report['source_after'] = {s: source_identity(bindings['identities'][s]['root']) for s in SIDES}
            event('run-finished', status=report['status'], new_completed_pairs=report['new_completed_pairs'])
            checkpoint()
        finally:
            journal.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', required=True, type=Path)
    parser.add_argument('--config-sha256', required=True)
    parser.add_argument('--execute', action='store_true')
    parser.add_argument('--dry-run', action='store_true')
    args = parser.parse_args()
    path = args.config.resolve()
    config, bindings, amendment = validate(path, args.config_sha256)
    launcher, driver, observer = commands(config, path, args.config_sha256)
    intent_path, start_path = LOCAL / (RUN + '-launch.json'), LOCAL / (RUN + '-driver-start.json')
    config_pin = {'path': str(path), 'sha256': args.config_sha256}
    if args.dry_run:
        require(not args.execute, 'Dry run cannot execute')
        print(json.dumps({'launcher': launcher, 'driver': driver, 'observer': observer, 'launches_nothing': True}, indent=2))
        return
    receipt_path = LOCAL / (RUN + '-receipt.json')
    require(os.environ.get('PAGURA_RUN_RECEIPT') == str(receipt_path), 'Fixed owned receipt required')
    live = load(receipt_path)
    require(live['status'] == 'running' and live['run_id'] == os.environ.get('PAGURA_RUN_ID') and live['argv'] == observer
            and live['source_root'] == str(ROOT) and live['source_before'] == bindings['runner_sources']['candidate'], 'Actual runner/observer differs')
    if not args.execute:
        require(sys.orig_argv == launcher and not any(p.exists() for p in (intent_path, start_path, LOCAL / RUN)), 'Already launched or wrong launcher argv')
        with intent_path.open('x') as output:
            json.dump({'schema': 1, 'experiment': NAME, 'status': 'launch-intent-not-completion', 'config': config_pin,
                       'argv': launcher, 'command': driver, 'run_id': live['run_id'], 'run_receipt': str(receipt_path)}, output, indent=2)
        os.environ['PAGURA_F32_CONTINUATION_INTENT_SHA256'] = descriptor(intent_path)['sha256']
        os.execv(config['python']['path'], driver)
    intent_pin = {'path': str(intent_path), 'sha256': os.environ.get('PAGURA_F32_CONTINUATION_INTENT_SHA256')}
    intent = pin(intent_pin)
    require(sys.orig_argv == driver and intent['argv'] == launcher and intent['command'] == driver
            and intent['config'] == config_pin and intent['run_id'] == live['run_id'] and intent['run_receipt'] == str(receipt_path), 'Post-exec driver identity differs')
    with start_path.open('x') as output:
        json.dump({'schema': 1, 'experiment': NAME, 'status': 'driver-start-not-completion', 'config': config_pin,
                   'intent': intent_pin, 'argv': sys.orig_argv, 'run_id': live['run_id'], 'run_receipt': str(receipt_path)}, output, indent=2)
    execute(config, bindings, amendment, config_pin, intent_pin, descriptor(start_path))


if __name__ == '__main__':
    handlers = {s: signal.getsignal(s) for s in (signal.SIGINT, signal.SIGTERM)}
    def cancel(number, _frame):
        raise MeasurementCancelled(number)
    try:
        for number in handlers:
            signal.signal(number, cancel)
        main()
    finally:
        for number, handler in handlers.items():
            signal.signal(number, handler)
