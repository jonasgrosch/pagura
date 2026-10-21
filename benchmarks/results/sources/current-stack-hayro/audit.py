#!/usr/bin/env python3
"""Independent retained-evidence audit of the fixed current-stack/Hayro panel.

Never renders or imports the producer. The pinned historical analytic19 auditor
supplies only its independent image parser/comparator and safe byte/JSON readers.
All failed or mismatching attempts remain represented; no release admission.
"""
import argparse
from collections import Counter
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import random
import re
import stat
import statistics
import sys

sys.dont_write_bytecode = True
NAME = 'current-stack-hayro'
SAMPLES, WARMUPS, SEED = 100, 5, 20261022
SIDES = ('pagura', 'hayro')
LIMIT = 128 * 1024 * 1024
HISTORICAL_SHA = 'b135b3804ae49fb66c650f13c7d1aa883651a8f63d1ac0e44b4badcf485a32ff'
REFERENCES_SHA = '924e20d6495990e8206c837d420c0172d5404ea1b3ede56417a9d59bb84bbcaf'


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def read(path, limit=LIMIT):
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | getattr(os, 'O_NOFOLLOW', 0))
    with os.fdopen(fd, 'rb') as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_size <= limit, 'Expected bounded regular evidence: ' + str(path))
        raw = stream.read(limit + 1)
        require(len(raw) <= limit, 'Evidence exceeds bound')
        return raw


def file_hash(path):
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | getattr(os, 'O_NOFOLLOW', 0))
    with os.fdopen(fd, 'rb') as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_size <= 1024**3, 'Expected bounded regular identity')
        digest, count = hashlib.sha256(), 0
        while raw := stream.read(1024 * 1024):
            count += len(raw)
            require(count <= 1024**3, 'Identity exceeds bound')
            digest.update(raw)
        return digest.hexdigest()


def decode(raw):
    def pairs(values):
        result = {}
        for key, value in values:
            require(key not in result, 'Duplicate JSON field')
            result[key] = value
        return result
    return json.loads(raw, object_pairs_hook=pairs,
                      parse_constant=lambda x: (_ for _ in ()).throw(ValueError('Nonfinite JSON: ' + x)))


def descriptor(path):
    return {'path': str(Path(path).resolve()), 'sha256': file_hash(path)}


def pinned(item):
    raw = read(Path(item['path']))
    require(sha(raw) == item['sha256'], 'Independent artifact pin differs: ' + item['path'])
    if 'bytes' in item:
        require(type(item['bytes']) is int and len(raw) == item['bytes'], 'Artifact byte length differs')
    return decode(raw)


def load_images(root):
    path = root / 'benchmarks/results/sources/analytic19-conversion/audit.py'
    require(file_hash(path) == HISTORICAL_SHA, 'Independent historical image parser changed')
    spec = importlib.util.spec_from_file_location('independent_analytic19_images', path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def parse_stages(log, elapsed):
    records = []
    for line in log.splitlines():
        if line.startswith('{'):
            value = decode(line.encode())
            if isinstance(value, dict) and ('open_ns' in value or 'pages' in value):
                records.append(value)
    if not records:
        return None
    require(len(records) == 1, 'Ambiguous timing records')
    value = records[0]
    pages = value.get('pages')
    require(type(value.get('schema')) is int and value['schema'] == 1 and isinstance(pages, list)
            and len(pages) == 1 and isinstance(pages[0], dict)
            and type(pages[0].get('page')) is int and pages[0]['page'] == 0,
            'Timing schema/page differs')
    result = {k: value.get(k) for k in ('open_ns', 'total_ns')}
    result.update({k: pages[0].get(k) for k in ('render_ns', 'emit_ns')})
    require(all(type(v) is int and v >= 0 for v in result.values()), 'Invalid stage nanoseconds')
    result['engine_ns'] = result['open_ns'] + result['render_ns']
    require(type(elapsed) is int and elapsed > 0
            and result['engine_ns'] + result['emit_ns'] <= result['total_ns'] <= elapsed,
            'Stage accounting exceeds process time')
    return result


def distribution(values):
    ordered = sorted(values)
    location = .95 * (len(ordered) - 1)
    low, high = math.floor(location), math.ceil(location)
    return {'samples': len(values), 'minimum': ordered[0], 'median': statistics.median(ordered),
            'p95': ordered[low] + (ordered[high] - ordered[low]) * (location - low), 'maximum': ordered[-1]}


def metrics(pairs, qualified):
    result = {}
    for metric in ('engine_ns', 'render_ns', 'elapsed_ns', 'open_ns', 'emit_ns', 'total_ns', 'peak_rss_bytes'):
        sides, available = {}, {}
        for side in SIDES:
            values = [pair[side]['metrics'].get(metric) for pair in pairs]
            valid = [v for v in values if type(v) is int and v >= 0]
            available[side] = len(valid)
            if len(values) == SAMPLES and len(valid) == SAMPLES:
                sides[side] = distribution(values)
        record = {'status': 'unavailable-incomplete-samples' if len(sides) != 2 else
                  ('accuracy-qualified' if qualified else 'quality-unqualified'),
                  'available_samples': available, 'sides': sides}
        if len(sides) == 2:
            record['ratio_direction'] = 'pagura/hayro; lower is faster or smaller'
            record['ratios'] = {k: sides['pagura'][k] / sides['hayro'][k] if sides['hayro'][k] > 0 else None
                                for k in ('median', 'p95', 'maximum')}
        result[metric] = record
    return result


def successful_receipt(receipt):
    require(receipt.get('status') == 'completed' and receipt.get('source_unchanged') is True
            and receipt.get('source_before') == receipt.get('source_after')
            and not receipt.get('errors') and not receipt.get('artifact_errors')
            and receipt.get('cancellation_signal') is None
            and all(type(receipt.get(k)) is int and receipt[k] == 0 for k in ('exit_code', 'wrapper_exit_code')),
            'Receipt is not completed and source unchanged')


def strict_guard(guard):
    require(guard.get('schema') == 1 and guard.get('status') == 'completed'
            and guard.get('observation_only') is False and not guard.get('cancellation_requested_utc')
            and all(type(guard.get(k)) is int and guard[k] == 0 for k in ('exit_code', 'child_exit_code'))
            and isinstance(guard.get('events'), list)
            and {'preflight', 'postflight'} <= {v['stage'] for v in guard['events']}
            and all(v['matches'] == [] for v in guard['events']), 'Observer did not complete cleanly')


def check_arm(record, side, mode, case, config, directory, images, expected, blobs):
    output = directory / 'pending.raw'
    binary, pdf = config['engines'][side]['path'], case['pdf']['path']
    command = ([binary, 'render', '--timings', '--accuracy', mode, pdf, str(output), '72', '0']
               if side == 'pagura' else [binary, pdf, str(output), '72', '0'])
    require(record['command'] == command, 'Actual renderer argv differs')
    process, diagnostic = record['process'], record['diagnostic']
    require(isinstance(diagnostic, str) and '\ufffd' not in diagnostic
            and type(process.get('elapsed_ns')) is int and process['elapsed_ns'] > 0,
            'Malformed/truncated diagnostic or process time')
    rss = re.findall(r'^\s*(\d+)\s+maximum resident set size\s*$', diagnostic, re.MULTILINE)
    require(len(rss) <= 1, 'Ambiguous per-child RSS')
    peak = int(rss[0]) if rss else None
    require(process.get('peak_rss_bytes') == peak and (peak is None or peak > 0), 'RSS bytes differ from raw Darwin log')
    timed_out = process.get('status') == 'timeout'
    process_ok = type(process.get('exit_code')) is int and process['exit_code'] == 0 and not timed_out
    stage, timing_error = None, None
    if process_ok:
        try:
            stage = parse_stages(diagnostic, process['elapsed_ns'])
            require(stage is not None, 'missing stages')
        except ValueError as error:
            timing_error = str(error)
        require(record.get('stages') == stage and bool(record.get('timing_error')) == bool(timing_error),
                'Retained log and parsed stage availability differ')
    else:
        require(record.get('stages') is None, 'Failed process unexpectedly claimed stage timing')
    actual, difference = None, None
    artifact = record.get('output')
    if artifact is not None:
        path = Path(artifact['path'])
        digest = artifact['sha256']
        require(isinstance(digest, str) and len(digest) == 64 and all(c in '0123456789abcdef' for c in digest)
                and path == directory / 'blobs' / (digest + '.raw'), 'Output artifact path is not content addressed')
        key = (str(path), case['oracle']['sha256'])
        if key not in blobs:
            raw = read(path)
            require(sha(raw) == digest and type(artifact['bytes']) is int and len(raw) == artifact['bytes'], 'Raw output pin differs')
            try:
                parsed = images.image(raw)
                comparison = images.compare(parsed, expected)
                # Cache only verified metadata, not another page-sized RGB
                # buffer for every engine/mode/content-addressed output.
                blobs[key] = ((parsed[0], parsed[1], sha(parsed[2]), parsed[3]), comparison, None)
            except ValueError as error:
                blobs[key] = (None, None, str(error))
        actual, difference, invalid = blobs[key]
    else:
        invalid = 'no retained output'
    status = ('process-failure' if not process_ok else 'invalid-output' if actual is None else
              'correct' if difference['status'] == 'correct' else 'incorrect')
    require(record['accuracy'] == status, 'Producer accuracy classification differs')
    if actual is not None and process_ok:
        require((record.get('width'), record.get('height')) == actual[:2]
                and record.get('pixels_sha256') == actual[2], 'Output pixel metadata differs')
        if 'difference' in record:
            projected = {'dimension_match': actual[:2] == expected[:2]}
            if projected['dimension_match']:
                projected.update({k: difference[k] for k in ('different_channels', 'maximum_channel_error')})
            require(record['difference'] == projected, 'Producer difference counts differ')
    values = {'elapsed_ns': process['elapsed_ns'], 'peak_rss_bytes': peak}
    if stage is not None:
        values.update(stage)
    return {'status': status, 'process_status': process.get('status'), 'exit_code': process.get('exit_code'),
            'metrics': values, 'difference': difference,
            'alpha': actual[3] if actual is not None else {'status': 'unavailable', 'reason': invalid},
            'output': artifact, 'stage_status': 'available' if stage is not None else 'unavailable',
            'timing_error': timing_error}


def schedule(cases):
    rng = random.Random(SEED)
    cells = [(case['id'], mode) for case in cases for mode in ('exact', 'competitive')]
    rng.shuffle(cells)
    result = []
    for index, (case, mode) in enumerate(cells):
        orders = []
        for number in range(-WARMUPS, SAMPLES):
            order = list(SIDES)
            rng.shuffle(order)
            orders.append((number, order))
        result.append((index, case, mode, orders))
    return result


def source_pins(config):
    require(isinstance(config['source_files'], dict) and config['source_files'], 'Missing source/runtime closure')
    for path, digest in config['source_files'].items():
        require(Path(path).is_absolute() and file_hash(path) == digest, 'Frozen source/runtime differs: ' + path)
    for identity in config['engines'].values():
        require(Path(identity['path']).is_absolute() and file_hash(identity['path']) == identity['sha256'], 'Frozen engine binary differs')


def inputs(root, config, images):
    references = pinned(config['legacy_references'])
    require(config['legacy_references']['sha256'] == REFERENCES_SHA and len(references['cases']) == 19,
            'Original independent legacy references changed')
    require(len(config['cases']) == 39 and len({c['id'] for c in config['cases']}) == 39
            and [c['cohort'] for c in config['cases']] == ['legacy19'] * 19 + ['interval20'] * 20,
            'Case inventory/order differs')
    sys.path.insert(0, str(root / 'tools'))
    sys.path.insert(0, str(root / 'benchmarks'))
    from benchkit.reference_cases import (validate_case_reference, load_reference_sources,
                                          invocation_settings, current_render_policy_sha256)
    import f32_painted_interval as fixtures
    require(len(fixtures.CASES) == 20, 'Interval fixture inventory differs')
    expected, profiles = {}, {}
    for index, case in enumerate(config['cases']):
        pdf, oracle = read(case['pdf']['path']), read(case['oracle']['path'])
        require(sha(pdf) == case['pdf']['sha256'] and sha(oracle) == case['oracle']['sha256'], 'Frozen input bytes differ')
        parsed = images.image(oracle)
        require(parsed[:2] == (case['width'], case['height']), 'Oracle dimensions differ')
        if index < 19:
            old = references['cases'][index]
            require(case['id'] == old['id'] and old['dpi'] == 72 and old['pages'] == 1 and old.get('tolerance', 0) == 0
                    and old['sha256'] == case['pdf']['sha256'] and old['oracle_sha256'] == case['oracle']['sha256']
                    and parsed[:2] == (old['width'], old['height']), 'Legacy input/reference selection differs')
            if 'render_references' in old:
                metadata, digest = validate_case_reference(old, 'exact')
                load_reference_sources(root, metadata)
                settings = invocation_settings(old['sha256'], page=0, dpi=72, accuracy='exact',
                    entrypoint='render_page', validation='full', render_policy_sha256=current_render_policy_sha256(root))
                require(settings == metadata['settings'] and metadata['oracle_sha256'] == sha(oracle),
                        'Legacy Exact reference source/settings differ')
                profiles[case['id']] = {'exact': 'source-and-settings-verified', 'sha256': digest,
                    'competitive': 'same frozen common-policy RGB oracle; not drawing-only reference attestation',
                    'hayro': 'same frozen policy comparison; disagreement is not alone a PDF violation'}
        else:
            require(case['id'] == 'interval/' + fixtures.CASES[index - 19], 'Interval case identity differs')
            generated_pdf, generated_oracle = fixtures.fixture(fixtures.CASES[index - 19])
            require(pdf == generated_pdf and oracle == generated_oracle, 'Frozen interval fixture recipe differs')
        expected[case['id']] = parsed
    return expected, profiles


class Journal:
    def __init__(self, path):
        fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | getattr(os, 'O_NOFOLLOW', 0))
        self.stream = os.fdopen(fd, 'rb')
        metadata = os.fstat(self.stream.fileno())
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_size <= 1024**3, 'Unbounded journal')
        self.previous, self.count = 0, 0

    def event(self, kind, **fields):
        raw = self.stream.readline(1024 * 1024 + 1)
        require(raw.endswith(b'\n') and len(raw) <= 1024 * 1024, 'Missing/oversized journal event')
        record = decode(raw)
        tick = record.pop('monotonic_ns')
        require(type(tick) is int and tick >= self.previous and record == {'event': kind, **fields},
                'Journal event/order differs')
        self.previous, self.count = tick, self.count + 1

    def finish(self):
        require(self.stream.read(1) == b'', 'Journal contains unaccounted attempts')
        self.stream.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--config-sha256', required=True)
    args = parser.parse_args()
    config_pin = {'path': str(args.config.resolve()), 'sha256': args.config_sha256}
    config = pinned(config_pin)
    root = Path(config['product']['root']).resolve()
    package = root / 'benchmarks/results/sources/current-stack-hayro'
    local = root / 'benchmarks/results/local/current-stack-hayro'
    directory = local / 'run-001'
    require(Path.cwd().resolve() == root and args.config.resolve() == local / 'config.json'
            and config['schema'] == 1 and config['experiment'] == NAME
            and config['status'] == 'frozen-before-measurement', 'Wrong config/root')
    protocol = config['protocol']
    require((protocol['samples'], protocol['warmups'], protocol['seed'], protocol['modes'], protocol['dpi'], protocol['page'])
            == (100, 5, 20261022, ['exact', 'competitive'], 72, 0)
            and protocol['expected_cells'] == 78 and protocol['expected_outputs'] == 16380
            and protocol['primary_metric'] == 'engine_ns' and protocol['release_admission'] is False
            and protocol['retry'] is False and protocol['renderer_changes'] is False, 'Fixed protocol differs')
    require(set(config['engines']) == set(SIDES), 'Engine inventory differs')
    source_pins(config)
    require(config['runtime']['path'] == str(Path(sys.executable).resolve())
            and file_hash(config['runtime']['path']) == config['runtime']['sha256']
            and config['launcher']['path'] == str(Path(sys.orig_argv[0]).resolve())
            and file_hash(config['launcher']['path']) == config['launcher']['sha256'], 'Python launcher/reported runtime differs')
    for required in (package / 'run.py', package / 'prepare.py',
                     root / 'benchmarks/results/sources/analytic19-conversion/audit.py'):
        require(str(required) in config['source_files'], 'Required audit/producer source not frozen')
    images = load_images(root)
    expected, profiles = inputs(root, config, images)
    from paired_render import source_identity
    from benchkit.config import source_digest
    require(source_identity(root) == config['product'] and source_digest() == config['runner_source_sha256'], 'Current source differs')
    build = {k: pinned(v) for k, v in config['build'].items()}
    successful_receipt(build['receipt'])
    strict_guard(build['guard'])
    require(build['report']['status'] == 'completed'
            and build['report']['source_before'] == build['report']['source_after'] == config['product']
            and build['receipt']['source_before']['sha256'] == config['runner_source_sha256']
            and build['receipt']['source_root'] == str(root)
            and build['report']['run_id'] == build['receipt']['run_id']
            and build['report']['run_receipt'] == config['build']['receipt']['path'], 'Build source/run differs')
    require([v['engine'] for v in build['report']['builds']] == list(SIDES), 'Build engine inventory differs')
    for record in build['report']['builds']:
        identity = config['engines'][record['engine']]
        require(type(record['exit_code']) is int and record['exit_code'] == 0
                and record['artifact'] == {k: identity[k] for k in ('path', 'sha256')}
                and record['binary'] == identity['path'] and record['version'] == identity['version']
                and build['receipt']['artifacts'].get(identity['path']) == identity['sha256'], 'Build/binary correspondence differs')
        command = record['command']
        if record['engine'] == 'pagura':
            require(len(command) == 11 and command[1:9] == ['build', '--locked', '--offline', '--release',
                    '-p', 'pagura', '--target-dir', str(Path(identity['path']).parents[1])]
                    and command[9:] == ['-j', '1'], 'Pagura build profile differs')
        else:
            require(command[1:8] == ['+1.99.1', 'build', '--locked', '--offline', '--release', '--manifest-path',
                    str(Path(record['cwd']) / 'Cargo.toml')]
                    and command[8:] == ['--target-dir', str(Path(identity['path']).parents[1]), '-j', '1']
                    and identity['version'].startswith('hayro 0.8.0;'), 'Hayro build profile/version differs')
            for relative in ('Cargo.toml', 'Cargo.lock', 'src/main.rs'):
                require(read(Path(record['cwd']) / relative) == read(root / 'benchmarks/adapters/hayro' / relative),
                        'Built Hayro adapter differs from frozen repository adapter')
    for key in ('report', 'guard'):
        require(build['receipt']['artifacts'].get(config['build'][key]['path']) == config['build'][key]['sha256'], 'Build receipt misses evidence')
    build_command = [config['launcher']['path'], str(package / 'prepare.py'), 'build']
    observer = root / 'benchmarks/results/sources/analytic19-conversion/load_observer.py'
    require(build['report']['argv'] == build_command and build['guard']['command'] == build_command
            and build['receipt']['argv'] == [config['launcher']['path'], str(observer), '--evidence-dir',
                 str(Path(config['build']['guard']['path']).parent), '--', *build_command], 'Build launch chain differs')
    receipt_pin, guard_pin, report_pin = (descriptor(p) for p in (
        local / 'run-001-receipt.json', local / 'run-001-observer/guard.json', directory / 'report.json'))
    receipt, guard, report = (pinned(p) for p in (receipt_pin, guard_pin, report_pin))
    successful_receipt(receipt)
    strict_guard(guard)
    command = [config['launcher']['path'], str(package / 'run.py'), '--config', config_pin['path'],
               '--config-sha256', config_pin['sha256']]
    require(str(observer) in config['source_files'] and guard['command'] == command
            and receipt['argv'] == [config['launcher']['path'], str(observer), '--evidence-dir',
                                   str(local / 'run-001-observer'), '--', *command], 'Actual observer/driver chain differs')
    require(receipt['source_root'] == str(root) and receipt['source_before']['sha256'] == config['runner_source_sha256']
            and receipt['source_before']['commit'] == config['product']['commit']
            and report['source_before'] == report['source_after'] == config['product']
            and report['run_id'] == receipt['run_id'] and report['run_receipt'] == receipt_pin['path']
            and report['argv'] == command and report['config'] == config_pin, 'Measurement source/run/config differs')
    require(report['schema'] == 1 and report['experiment'] == NAME and report['status'] == 'completed'
            and report['release_admission'] is False and report['completed_pairs'] == 8190
            and len(report['cells']) == 78, 'Measurement is not the complete fixed panel')
    journal_pin = descriptor(directory / 'journal.jsonl')
    require(report['journal'] == journal_pin, 'Report journal pin differs')
    for item in (config_pin, guard_pin, report_pin, journal_pin):
        require(receipt['artifacts'].get(item['path']) == item['sha256'], 'Final receipt misses required evidence')
    journal = Journal(Path(journal_pin['path']))
    journal.event('run-start', config=config_pin, run_id=report['run_id'])
    cases = {c['id']: c for c in config['cases']}
    cells, blobs, attempts = [], {}, 0
    all_counts = {side: Counter() for side in SIDES}
    for index, name, mode, orders in schedule(config['cases']):
        raw_cell, case = report['cells'][index], cases[name]
        require((raw_cell['case'], raw_cell['cohort'], raw_cell['mode']) == (name, case['cohort'], mode)
                and len(raw_cell['pairs']) == 105, 'Cell order/coverage differs')
        observations, failures, counts = [], [], {side: Counter() for side in SIDES}
        differences, alphas = {side: {} for side in SIDES}, {side: {} for side in SIDES}
        for raw_pair, (number, order) in zip(raw_cell['pairs'], orders):
            require(raw_pair['pair'] == number and raw_pair['order'] == order
                    and type(raw_pair['started_monotonic_ns']) is int and type(raw_pair['finished_monotonic_ns']) is int
                    and raw_pair['started_monotonic_ns'] <= raw_pair['finished_monotonic_ns'], 'Pair order/completion differs')
            journal.event('pair-start', cell_index=index,
                          sample={k: raw_pair[k] for k in ('pair', 'order', 'started_monotonic_ns')})
            observation = {'pair': number}
            for side in order:
                raw_arm = raw_pair[side]
                journal.event('arm-result', cell_index=index, pair=number, side=side, record=raw_arm)
                arm = check_arm(raw_arm, side, mode, case, config, directory, images, expected[name], blobs)
                observation[side] = arm
                counts[side][arm['status']] += 1
                all_counts[side][arm['status']] += 1
                key = raw_arm['output']['sha256'] if raw_arm.get('output') else 'no-output'
                differences[side][key] = arm['difference']
                alphas[side][key] = arm['alpha']
                if arm['status'] != 'correct' or arm['stage_status'] != 'available' or arm['metrics']['peak_rss_bytes'] is None:
                    failures.append({'pair': number, 'side': side, 'accuracy': arm['status'],
                        'exit_code': arm['exit_code'], 'process_status': arm['process_status'],
                        'stage_status': arm['stage_status'], 'timing_error': arm['timing_error'],
                        'rss_available': arm['metrics']['peak_rss_bytes'] is not None, 'output': arm['output']})
                attempts += 1
            journal.event('pair-complete', cell_index=index, sample=raw_pair)
            observations.append(observation)
        qualified = all(counts[s]['correct'] == 105 for s in SIDES)
        cells.append({'case': name, 'cohort': case['cohort'], 'mode': mode,
            'qualification': 'both-engines-correct' if qualified else 'quality-unqualified',
            'attempts_per_side': 105, 'measured_pairs': 100, 'warmup_pairs': 5,
            'accuracy_counts': {s: dict(counts[s]) for s in SIDES},
            'failed_mismatching_or_unavailable_attempts': failures,
            'differences_by_output': differences, 'alpha_by_output': alphas,
            'metrics': metrics(observations[5:], qualified),
            'timing_unavailable_counts': {s: sum(p[s]['stage_status'] == 'unavailable' for p in observations) for s in SIDES}})
    require(attempts == 16380, 'Output attempt count differs')
    journal.event('run-finished', status='completed', completed_pairs=8190)
    journal.finish()
    source_pins(config)
    require(source_identity(root) == config['product'] and source_digest() == config['runner_source_sha256'], 'Source changed during audit')
    for item in (config_pin, receipt_pin, guard_pin, report_pin, journal_pin, *config['build'].values()):
        require(descriptor(item['path']) == item, 'Evidence changed during audit')
    summary = {'schema': 1, 'experiment': NAME, 'status': 'verified-complete-panel',
        'config': config_pin, 'report': report_pin, 'receipt': receipt_pin, 'guard': guard_pin, 'journal': journal_pin,
        'audit_source': descriptor(Path(__file__).resolve()), 'product': config['product'], 'engines': config['engines'],
        'attempted_outputs': attempts, 'paired_positions': 8190, 'cell_count': 78,
        'qualified_cells': sum(c['qualification'] == 'both-engines-correct' for c in cells),
        'accuracy_counts': {s: dict(all_counts[s]) for s in SIDES},
        'distinct_raw_output_reference_checks': len(blobs), 'journal_events': journal.count,
        'reference_profiles': profiles, 'cells': cells, 'release_admission': False,
        'limitations': [
            'Fixed synthetic workload panel; no universal PDF correctness or global engine-superiority claim.',
            'Exact and Competitive are separate cells; Hayro is sampled afresh in both cells with identical settings.',
            'Primary metric is open plus render; render-only and full process timings are separately retained.',
            'All 100 measured pairs contribute or that metric is unavailable; wrong-output timings remain explicitly unqualified.',
            'All five warmups contribute to accuracy qualification; no sample is removed for an unfavorable value.',
            'RGB-over-white comparison uses frozen independent references at zero tolerance; policy disagreement alone is not a PDF violation.',
            'Pagura P6 carries no alpha; Hayro alpha is observed without an independent alpha-superiority claim.',
            'RSS is raw per-child Darwin /usr/bin/time maximum resident bytes; distributions are descriptive, with no confidence interval.',
            'Point ratios are descriptive, without bootstrap bounds or release admission.',
            'Retained diagnostics are the combined text returned by run_process, not separately captured original stdout/stderr streams.',
            'The cooperative observer cannot exclude ordinary desktop interference or activity between observations.']}
    output = local / 'run-001-analysis.json'
    with output.open('x') as stream:
        stream.write(json.dumps(summary, indent=2, allow_nan=False) + '\n')
    print(json.dumps({k: v for k, v in summary.items() if k != 'cells'}, indent=2))


if __name__ == '__main__':
    main()
