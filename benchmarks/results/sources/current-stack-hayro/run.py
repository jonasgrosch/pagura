"""One fixed paired CLI comparison; retain failures, pixels, logs and memory."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import signal
import sys
import time

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[4]
sys.path[:0] = [str(ROOT / 'benchmarks'), str(ROOT / 'tools')]
from paired_render import source_identity
from benchkit.artifacts import hash_regular_file, read_bounded_regular
from benchkit.config import source_digest
from benchkit.images import normalize
from benchkit.measurement import MeasurementCancelled, run_process
from benchkit.timings import parse_stages

MAX_BYTES = 64 * 1024 * 1024
MAX_RETAINED = 1024 * 1024 * 1024


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def pin(path):
    path = Path(path).resolve()
    return {'path': str(path), 'sha256': hash_regular_file(path)}


def verify(config):
    assert source_identity(ROOT) == config['product']
    assert source_digest() == config['runner_source_sha256']
    for path, digest in config['source_files'].items():
        assert hash_regular_file(path) == digest, path
    for item in (*config['engines'].values(), *config['build'].values(), config['runtime'], config['launcher'], config['policy']):
        assert pin(item['path'])['sha256'] == item['sha256'], item['path']
    for case in config['cases']:
        for key in ('pdf', 'oracle'):
            assert pin(case[key]['path']) == case[key]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--config-sha256', required=True)
    args = parser.parse_args()
    config_pin = {'path': str(args.config.resolve()), 'sha256': args.config_sha256}
    assert pin(args.config) == config_pin
    config = json.loads(args.config.read_bytes())
    assert pin(sys.orig_argv[0]) == config['launcher']
    assert pin(sys.executable) == config['runtime']
    verify(config)
    assert config['status'] == 'frozen-before-measurement'
    protocol = config['protocol']
    assert (protocol['samples'], protocol['warmups'], protocol['seed']) == (100, 5, 20261022)
    assert len(config['cases']) == 39 and protocol['modes'] == ['exact', 'competitive']
    directory = args.config.parent / 'run-001'
    directory.mkdir(exist_ok=False)
    journal_path = directory / 'journal.jsonl'
    report = {'schema': 1, 'experiment': 'current-stack-hayro', 'config': config_pin,
        'status': 'running', 'run_id': os.environ['PAGURA_RUN_ID'],
        'run_receipt': os.environ['PAGURA_RUN_RECEIPT'], 'argv': sys.orig_argv,
        'source_before': source_identity(ROOT), 'source_after': None, 'cells': [],
        'environment': {'platform': platform.platform(), 'python': sys.version, 'machine': platform.machine()},
        'release_admission': False, 'completed_pairs': 0}
    stored, accuracy_cache = {}, {}
    expected = {c['id']: normalize(read_bounded_regular(c['oracle']['path'], MAX_BYTES)) for c in config['cases']}
    cells = [(case, mode) for case in config['cases'] for mode in protocol['modes']]
    rng = random.Random(protocol['seed'])
    rng.shuffle(cells)
    output = directory / 'pending.raw'
    policy = {'timeout_seconds': 30, 'maximum_output_bytes': MAX_BYTES, 'measure_rss': True}

    def retain(raw):
        digest = sha(raw)
        if digest not in stored:
            assert sum(v['bytes'] for v in stored.values()) + len(raw) <= MAX_RETAINED
            path = directory / 'blobs' / (digest + '.raw')
            path.parent.mkdir(exist_ok=True)
            with path.open('xb') as stream:
                stream.write(raw)
            path.chmod(0o400)
            stored[digest] = {'path': str(path), 'sha256': digest, 'bytes': len(raw)}
        return stored[digest]

    def checkpoint():
        pending = directory / 'report.pending'
        pending.write_text(json.dumps(report, indent=2, allow_nan=False) + '\n')
        pending.replace(directory / 'report.json')

    with journal_path.open('x') as journal:
        def event(kind, **values):
            journal.write(json.dumps({'event': kind, 'monotonic_ns': time.monotonic_ns(), **values}, allow_nan=False) + '\n')
            journal.flush()
        try:
            event('run-start', config=config_pin, run_id=report['run_id'])
            checkpoint()
            for case, mode in cells:
                cell = {'case': case['id'], 'cohort': case['cohort'], 'mode': mode, 'pairs': []}
                report['cells'].append(cell)
                for number in range(-5, 100):
                    order = ['pagura', 'hayro']
                    rng.shuffle(order)
                    pair = {'pair': number, 'order': order, 'started_monotonic_ns': time.monotonic_ns()}
                    cell['pairs'].append(pair)
                    event('pair-start', cell_index=len(report['cells']) - 1, sample=pair)
                    for side in order:
                        engine = config['engines'][side]
                        assert pin(engine['path'])['sha256'] == engine['sha256']
                        assert pin(case['pdf']['path']) == case['pdf']
                        output.unlink(missing_ok=True)
                        command = ([engine['path'], 'render', '--timings', '--accuracy', mode]
                                   if side == 'pagura' else [engine['path']])
                        command += [case['pdf']['path'], str(output), '72', '0']
                        record = {'command': command, 'output': None, 'accuracy': 'process-failure'}
                        pair[side] = record
                        try:
                            process, diagnostic = run_process(command, policy)
                            record.update(process=process, diagnostic=diagnostic)
                            if output.exists():
                                raw = read_bounded_regular(output, MAX_BYTES)
                                record['output'] = retain(raw)
                            if process['exit_code'] == 0 and process['status'] != 'timeout':
                                try:
                                    assert record['output'] is not None, 'missing output'
                                    key = (record['output']['sha256'], case['oracle']['sha256'])
                                    if key not in accuracy_cache:
                                        actual = normalize(raw)
                                        want = expected[case['id']]
                                        diff = {'dimension_match': actual[:2] == want[:2]}
                                        if diff['dimension_match']:
                                            diff['different_channels'] = sum(a != b for a, b in zip(actual[2], want[2])) if actual != want else 0
                                            diff['maximum_channel_error'] = max((abs(a - b) for a, b in zip(actual[2], want[2])), default=0) if actual != want else 0
                                        accuracy_cache[key] = {'accuracy': 'correct' if actual == want else 'incorrect',
                                            'pixels_sha256': sha(actual[2]), 'width': actual[0], 'height': actual[1], 'difference': diff}
                                    record.update(accuracy_cache[key])
                                except (ValueError, AssertionError) as error:
                                    record.update(accuracy='invalid-output', output_error=str(error))
                            if process['exit_code'] == 0 and process['status'] != 'timeout':
                                try:
                                    stages = parse_stages(diagnostic, 0, process['elapsed_ns'])
                                    assert stages is not None, 'missing stages'
                                    record['stages'] = stages
                                except (ValueError, AssertionError) as error:
                                    record['timing_error'] = str(error)
                        except BaseException as error:
                            record['interruption'] = type(error).__name__ + ': ' + str(error)
                            if output.exists():
                                record['output'] = retain(read_bounded_regular(output, MAX_BYTES))
                            raise
                        finally:
                            event('arm-result', cell_index=len(report['cells']) - 1, pair=number, side=side, record=record)
                        output.unlink(missing_ok=True)
                    pair['finished_monotonic_ns'] = time.monotonic_ns()
                    report['completed_pairs'] += 1
                    event('pair-complete', cell_index=len(report['cells']) - 1, sample=pair)
                checkpoint()
                print(f"completed {len(report['cells'])}/78: {case['id']}/{mode}", flush=True)
            verify(config)
            for item in stored.values():
                assert pin(item['path'])['sha256'] == item['sha256']
            assert report['completed_pairs'] == 8190
            report['status'] = 'completed'
        except BaseException as error:
            report.update(status='incomplete', error=type(error).__name__ + ': ' + str(error))
            raise
        finally:
            report['source_after'] = source_identity(ROOT)
            report['stored_bytes'] = sum(v['bytes'] for v in stored.values())
            event('run-finished', status=report['status'], completed_pairs=report['completed_pairs'])
            journal.flush()
            report['journal'] = pin(journal_path)
            checkpoint()


if __name__ == '__main__':
    def cancel(number, _frame):
        raise MeasurementCancelled(number)
    for number in (signal.SIGINT, signal.SIGTERM):
        signal.signal(number, cancel)
    main()
