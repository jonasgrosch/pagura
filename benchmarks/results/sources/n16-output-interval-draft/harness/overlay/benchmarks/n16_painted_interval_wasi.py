#!/usr/bin/env python3
"""Fixed twelve-cell painted-interval WASI triage; no speed admission.

Run once under the coordinator's serial runner/observer after both WASM builds.
Every invocation uses a fresh default-tiering Node process and WASM instance.
Only the unchanged Rust example's render_ns is the diagnostic latency metric.
"""
import argparse
import hashlib
import json
import os
import platform
import random
import signal
import statistics
import subprocess
import sys
import time
from pathlib import Path

sys.dont_write_bytecode = True
SEED, PAIRS, WARMUPS, DPI = 20261107, 12, 2, 72
N16_CASES = ('opaque64', 'opaque-fill', 'opaque-empty', 'opaque-distant', 'opaque-tiny')
CELLS = (('opaque64', 'exact'), ('opaque64', 'competitive'),
         ('opaque-fill', 'exact'), ('opaque-fill', 'competitive'),
         ('opaque-empty', 'exact'), ('opaque-empty', 'competitive'),
         ('opaque-distant', 'exact'), ('opaque-distant', 'competitive'),
         ('opaque-tiny', 'exact'), ('opaque-tiny', 'competitive'),
         ('image-only', 'exact'), ('default-knockout64', 'exact'))
HARNESS = ('n16_painted_interval.py', 'n16_painted_interval_fixtures.py',
           'n16_output_u16.py', 'n16_output_guarded.py', 'n16_output.py',
           'opaque_composition.py', 'paired_render.py', 'glyph_memo.py',
           'glyph_memo_fixtures.py', 'scene_index.py', 'clip_viewport_fixtures.py')
MAX_BYTES, MAX_RETAINED = 64 * 1024 * 1024, 512 * 1024 * 1024


def finish_attempt(sample, paths, record_existing):
    """Retain bounded files left by a failed/interrupted owned child too."""
    errors = {}
    for path in paths:
        if path.exists():
            try:
                record_existing(path)
            except (OSError, ValueError) as error:
                errors[str(path)] = f'{type(error).__name__}: {error}'
    if errors:
        sample['artifact_errors'] = errors
    sample['finished_monotonic_ns'] = time.monotonic_ns()
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('baseline-source', 'candidate-source', 'baseline-wasm', 'candidate-wasm',
                 'baseline-build-receipt', 'candidate-build-receipt', 'bindings',
                 'build-bindings', 'plan', 'node', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    for name in ('bindings', 'build-bindings', 'plan'):
        parser.add_argument('--' + name + '-sha256', required=True)
    parser.add_argument('--run-id', required=True)
    args = parser.parse_args()
    if not args.run_id or args.run_id != os.environ.get('PAGURA_RUN_ID') or not os.environ.get('PAGURA_RUN_RECEIPT'):
        parser.error('Run through the coordinator with a matching run id and receipt')
    if os.environ.get('NODE_OPTIONS') or os.environ.get('NODE_V8_COVERAGE'):
        parser.error('NODE_OPTIONS and NODE_V8_COVERAGE must be unset')
    roots = {side: getattr(args, side + '_source').resolve() for side in ('baseline', 'candidate')}
    sys.path.insert(0, str(roots['candidate'] / 'benchmarks'))
    import n16_painted_interval as experiment
    from paired_render import ROOT, expected_image, source_identity
    from benchkit.artifacts import hash_regular_file, read_bounded_regular
    from benchkit.images import normalize
    from benchkit.measurement import MeasurementCancelled, run_process
    from benchkit.statistics import quantile
    if ROOT.resolve() != roots['candidate'] or Path(experiment.__file__).resolve() != roots['candidate'] / 'benchmarks/n16_painted_interval.py':
        raise ValueError('Fixture/helper imports must belong to the recorded candidate source')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    wrapper = Path(__file__).resolve().with_name('run_profile_wasi.mjs')
    driver = Path(__file__).resolve()
    if driver != roots['candidate'] / 'benchmarks/n16_painted_interval_wasi.py':
        raise ValueError('WASI driver must belong to the bound candidate checkout')
    node = args.node.resolve(strict=True)
    bindings_path = args.bindings.resolve()
    sha = lambda data: hashlib.sha256(data).hexdigest()
    report = {'schema': 1, 'experiment': 'n16-painted-interval-wasi-diagnostic',
              'argv': sys.orig_argv,
              'run_id': args.run_id, 'status': 'incomplete', 'admission': False,
              'portable_promotion': 'not determined', 'started_at_unix_ns': time.time_ns(),
              'scope': 'Fresh default-tiering Node/WASI instance; internal render_ns; hashed warm filesystem',
              'limitations': [
                  'The unchanged profile uses default Repair open and full compile_page; diagnostics are not emitted.',
                  'Compile/instantiate and Node startup are excluded, but lazy compilation/tiering may occur inside the first render.',
                  'Twelve pairs provide diagnostic triage, not calibrated inference or a WASM speed/admission claim.',
                  'The native frozen gates remain unchanged and independently required.',
                  'Linear-memory end size captures instance growth through the whole command; it is not total Node memory or peak RSS.',
                  'Source-to-binary correspondence and equal compiler/profile/feature flags remain trusted coordinator build evidence.',
                  'No automatic retry; incomplete or contaminated evidence cannot clear portable promotion.',
              ],
              'coordination': {'run_id': os.environ.get('PAGURA_RUN_ID'),
                               'receipt': os.environ.get('PAGURA_RUN_RECEIPT')},
              'environment': {'platform': platform.platform(), 'machine': platform.machine(),
                              'python': sys.version, 'python_executable': str(Path(sys.executable).resolve())},
              'artifacts': {}, 'cells': [], 'verified_outputs': 0}
    stored = 0

    def retain(relative, data):
        nonlocal stored
        path = output / relative
        if path.exists():
            if read_bounded_regular(path, MAX_BYTES) != data:
                raise ValueError('Retained evidence changed: ' + relative)
        else:
            if stored + len(data) > MAX_RETAINED:
                raise ValueError('Diagnostic evidence capacity exhausted')
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open('xb') as handle:
                handle.write(data)
            path.chmod(0o400)
            stored += len(data)
        report['artifacts'][relative] = {'sha256': sha(data), 'bytes': len(data)}
        return path

    def record_existing(path):
        nonlocal stored
        data = read_bounded_regular(path, MAX_BYTES)
        relative = str(path.relative_to(output))
        if relative not in report['artifacts']:
            if stored + len(data) > MAX_RETAINED:
                raise ValueError('Diagnostic evidence capacity exhausted')
            stored += len(data)
        report['artifacts'][relative] = {'sha256': sha(data), 'bytes': len(data)}
        return data

    def checkpoint():
        report['stored_bytes'] = stored
        pending = output / 'report.pending'
        pending.write_text(json.dumps(report, indent=2) + '\n')
        pending.replace(output / 'report.json')

    previous_signals = {signum: signal.getsignal(signum) for signum in (signal.SIGINT, signal.SIGTERM)}
    cancellation_signal = None

    def cancel(signum, _frame):
        nonlocal cancellation_signal
        if cancellation_signal is None:
            cancellation_signal = signum
            raise MeasurementCancelled(signum)

    try:
        for signum in previous_signals:
            signal.signal(signum, cancel)
        checkpoint()
        bindings_raw = read_bounded_regular(bindings_path, MAX_BYTES)
        if sha(bindings_raw) != args.bindings_sha256:
            raise ValueError('Native binding independent pin mismatch')
        bindings = json.loads(bindings_raw)
        retain('bindings/native-bindings.json', bindings_raw)
        report['native_bindings'] = {'path': str(bindings_path), 'sha256': sha(bindings_raw)}
        frozen_paths = {}
        for label in ('build-bindings', 'plan'):
            path = getattr(args, label.replace('-', '_')).resolve()
            data = read_bounded_regular(path, MAX_BYTES)
            if sha(data) != getattr(args, label.replace('-', '_') + '_sha256'):
                raise ValueError('Independent pin mismatch: ' + label)
            retain('bindings/' + label + '.json', data)
            frozen_paths[path] = sha(data)
        builds = json.loads(read_bounded_regular(args.build_bindings, MAX_BYTES))
        frozen = json.loads(read_bounded_regular(args.plan, MAX_BYTES))
        if frozen != experiment.frozen_plan():
            raise ValueError('WASI and native frozen plans differ')
        if bindings['bound_files_sha256'].get('benchmarks/results/n16-painted-interval-plan.json') != args.plan_sha256:
            raise ValueError('Frozen plan is not bound by native identities')
        report['frozen_plan_sha256'] = args.plan_sha256
        report['build_bindings_sha256'] = args.build_bindings_sha256
        identities = {side: source_identity(root) for side, root in roots.items()}
        for side, identity in identities.items():
            if any(identity[key] != bindings['identities'][side][key]
                   for key in ('root', 'commit', 'source_sha256')):
                raise ValueError('Source does not match frozen native binding: ' + side)
            if any(identity[key] != frozen['identities_to_freeze'][side][key] for key in identity):
                raise ValueError('Source does not match frozen experiment plan: ' + side)
        dirty = subprocess.check_output(['git', '-C', str(roots['baseline']), 'status',
                                         '--porcelain=v1', '--untracked-files=all'], text=True)
        if dirty:
            raise ValueError('Baseline is not pristine')
        example = 'crates/pagura-render/examples/scene_index_profile.rs'
        if hash_regular_file(roots['baseline'] / example) != hash_regular_file(roots['candidate'] / example):
            raise ValueError('Profile example differs between builds')
        harness = {name: hash_regular_file(roots['candidate'] / ('benchmarks/' + name)) for name in HARNESS}
        for name, digest in harness.items():
            if bindings['bound_files_sha256'].get('benchmarks/' + name) != digest:
                raise ValueError('Fixture/shared harness differs from native binding: ' + name)
        report['harness_sha256'] = harness
        report['example_sha256'] = hash_regular_file(roots['candidate'] / example)
        report['identities'] = identities
        report['driver'] = {'path': str(driver), 'sha256': hash_regular_file(driver)}
        report['wrapper'] = {'path': str(wrapper), 'sha256': hash_regular_file(wrapper)}
        for path in (driver, wrapper):
            relative = path.relative_to(roots['candidate']).as_posix()
            if bindings['bound_files_sha256'].get(relative) != hash_regular_file(path):
                raise ValueError('Unbound WASI driver/wrapper: ' + relative)
        retain('bindings/diagnostic.py', read_bounded_regular(driver, MAX_BYTES))
        retain('bindings/run_profile_wasi.mjs', read_bounded_regular(wrapper, MAX_BYTES))
        report['node'] = {'path': str(node), 'sha256': hash_regular_file(node)}
        report['environment']['python_sha256'] = hash_regular_file(Path(sys.executable).resolve())
        modules = {}
        for side in roots:
            original = getattr(args, side + '_wasm').resolve()
            module_bytes = read_bounded_regular(original, MAX_BYTES)
            receipt_path = getattr(args, side + '_build_receipt').resolve()
            receipt_bytes = read_bounded_regular(receipt_path, MAX_BYTES)
            receipt = json.loads(receipt_bytes)
            build = builds[side]
            if (not isinstance(receipt, dict) or receipt.get('status') != 'completed'
                    or type(receipt.get('exit_code')) is not int or receipt['exit_code'] != 0
                    or type(receipt.get('wrapper_exit_code')) is not int or receipt['wrapper_exit_code'] != 0
                    or receipt.get('source_root') != str(roots[side])
                    or receipt.get('source_unchanged') is not True
                    or receipt.get('source_before') != receipt.get('source_after')
                    or receipt.get('source_before', {}).get('commit') != identities[side]['commit']
                    or receipt.get('source_before') != bindings['runner_sources'][side]
                    or receipt.get('source_before') != build['source']
                    or build['source_root'] != str(roots[side])
                    or build['path'] != str(original) or build['sha256'] != sha(module_bytes)
                    or build['receipt'] != str(receipt_path) or build['receipt_sha256'] != sha(receipt_bytes)
                    or receipt.get('cancellation_signal') is not None
                    or receipt.get('errors') or receipt.get('artifact_errors')
                    or receipt.get('artifacts', {}).get(str(original)) != sha(module_bytes)):
                raise ValueError('Missing successful build/module binding: ' + side)
            modules[side] = retain(f'bindings/{side}.wasm', module_bytes)
            retain(f'bindings/{side}-build-receipt.json', receipt_bytes)
            identities[side].update(module_path=str(original), module_sha256=sha(module_bytes),
                                    build_receipt_path=str(receipt_path), build_receipt_sha256=sha(receipt_bytes))
        rng = random.Random(SEED)
        plan = {'schema': 1, 'seed': SEED, 'pairs': PAIRS, 'warmups': WARMUPS, 'dpi': DPI,
                'outputs': len(CELLS) * (PAIRS + WARMUPS) * 2,
                'metric': 'render_ns', 'cells': [],
                'triage': {'pixel_mismatch': 'block portable candidate',
                           'n16_material_regression': {'median_ratio_gt': 1.05,
                               'paired_ratio_gt': 1.02, 'minimum_pairs_above': 10,
                               'action': 'suspend portable promotion; investigate or revise before native confirmation'},
                           'otherwise': 'No diagnostic suspension signal; no speed/admission claim'},
                'statistics': 'Ratios of marginal medians and linearly interpolated empirical p95; raw paired ratios. No bootstrap or confidence claim.'}
        for name, mode in CELLS:
            orders = [['baseline', 'candidate'] for _ in range(6)] + [['candidate', 'baseline'] for _ in range(6)]
            rng.shuffle(orders)
            warmups = [['baseline', 'candidate'], ['candidate', 'baseline']]
            rng.shuffle(warmups)
            plan['cells'].append({'case': name, 'mode': mode, 'operation': 'full',
                                  'n16': name in N16_CASES,
                                  'orders': warmups + orders})
        plan_path = retain('plan.json', (json.dumps(plan, indent=2) + '\n').encode())
        report['plan_sha256'] = hash_regular_file(plan_path)
        expected, inputs = {}, {}
        for name in dict.fromkeys(name for name, _ in CELLS):
            pdf, oracle = experiment.fixture(name)
            inputs[name] = retain(f'inputs/{name}.pdf', pdf)
            retain(f'oracles/{name}.ppm', oracle)
            expected[name] = expected_image(oracle, 'full')
        report['status'] = 'running'
        checkpoint()
        runtime = None
        policy = {'timeout_seconds': 120, 'maximum_output_bytes': MAX_BYTES, 'measure_rss': False}
        for cell_plan in plan['cells']:
            name, mode = cell_plan['case'], cell_plan['mode']
            cell = {key: value for key, value in cell_plan.items() if key != 'orders'}
            cell['pairs'] = []
            report['cells'].append(cell)
            for index, order in enumerate(cell_plan['orders']):
                pair = {'pair': index - WARMUPS, 'order': order,
                        'started_monotonic_ns': time.monotonic_ns()}
                cell['pairs'].append(pair)
                for side in order:
                    for path, digest in ((node, report['node']['sha256']),
                                         (wrapper, report['wrapper']['sha256']),
                                         (modules[side], identities[side]['module_sha256']),
                                         (inputs[name], report['artifacts'][f'inputs/{name}.pdf']['sha256'])):
                        if hash_regular_file(path) != digest:
                            raise ValueError('Frozen invocation input changed: ' + str(path))
                    sample_dir = output / 'samples' / f'{name}-{mode}-{index:02d}-{side}'
                    sample_dir.mkdir(parents=True)
                    raw_path = sample_dir / 'render.rgba'
                    stdout_path, stderr_path = sample_dir / 'wasi.stdout', sample_dir / 'wasi.stderr'
                    metadata_path = sample_dir / 'wrapper.json'
                    command = [str(node), str(wrapper), str(modules[side]), str(inputs[name]), mode,
                               str(raw_path), str(stdout_path), str(stderr_path), str(metadata_path)]
                    sample = {'command': command, 'status': 'running',
                              'started_monotonic_ns': time.monotonic_ns()}
                    pair[side] = sample
                    checkpoint()  # Attempt identity precedes invocation, including cancellation.
                    try:
                        process, diagnostic = run_process(command, policy)
                        sample.update(process=process, diagnostic=diagnostic)
                        retain(str((sample_dir / 'node.log').relative_to(output)), diagnostic.encode())
                        for path in (stdout_path, stderr_path, metadata_path):
                            if path.exists():
                                record_existing(path)
                        if process['exit_code'] != 0 or process['status'] == 'timeout':
                            raise ValueError('WASI invocation failed; no retry')
                        metadata = json.loads(record_existing(metadata_path))
                        if (metadata.get('status') != 'completed' or metadata.get('exit_code') != 0
                                or metadata.get('module_sha256') != identities[side]['module_sha256']
                                or metadata.get('input_sha256') != report['artifacts'][f'inputs/{name}.pdf']['sha256']
                                or metadata.get('wrapper_sha256') != report['wrapper']['sha256']
                                or Path(metadata['runtime']['executable']).resolve() != node
                                or metadata['runtime']['exec_argv'] != [] or metadata['runtime']['node_options']):
                            raise ValueError('WASI invocation/runtime binding mismatch')
                        if runtime is None:
                            runtime = metadata['runtime']
                            report['node']['runtime'] = runtime
                        elif metadata['runtime'] != runtime:
                            raise ValueError('Node runtime changed')
                        memory = metadata['linear_memory']
                        before, after = memory['before_start_bytes'], memory['after_start_bytes']
                        if not ((before is None and after is None) or
                                (type(before) is int and type(after) is int and 0 < before <= after
                                 and before % 65536 == 0 and after % 65536 == 0)):
                            raise ValueError('Invalid exported linear-memory sizes')
                        if record_existing(stderr_path):
                            raise ValueError('Unexpected WASI program diagnostics')
                        stages = json.loads(record_existing(stdout_path))
                        if (not isinstance(stages, dict) or any(type(stages.get(key)) is not int or stages[key] <= 0
                                for key in ('open_ns', 'compile_ns', 'render_ns', 'total_ns', 'metric_ns'))
                                or stages.get('schema') != 1 or stages.get('operation') != 'full'
                                or stages.get('accuracy') != mode or stages.get('dpi') != DPI
                                or (stages.get('width'), stages.get('height')) != expected[name][:2]
                                or stages['metric_ns'] != stages['render_ns']
                                or stages['total_ns'] < stages['open_ns'] + stages['compile_ns'] + stages['render_ns']):
                            raise ValueError('Invalid profile stages or settings')
                        sample.update(stages=stages, linear_memory=memory)
                        raw = read_bounded_regular(raw_path, MAX_BYTES)
                        blob = 'blobs/' + sha(raw) + '.rgba'
                        retain(blob, raw)
                        width, height, pixels = expected[name]
                        header = f'RGBA8 {width} {height} premultiplied\n'.encode()
                        try:
                            actual = normalize(raw)
                        except ValueError:
                            report['portable_promotion'] = 'blocked: malformed independent pixel output'
                            raise
                        equal = (raw.startswith(header) and raw[len(header) + 3::4] == b'\xff' * (width * height)
                                 and actual == expected[name])
                        sample.update(output_blob=blob, output_sha256=sha(raw),
                                      pixels_sha256=sha(actual[2]), oracle_equal=equal,
                                      width=actual[0], height=actual[1])
                        if not equal:
                            report['portable_promotion'] = 'blocked: independent pixel mismatch'
                            raise ValueError('Independent pixel mismatch')
                        raw_path.unlink()
                        report['verified_outputs'] += 1
                        sample['status'] = 'completed'
                    except BaseException as error:
                        if isinstance(error, (KeyboardInterrupt, SystemExit)):
                            cancellation_signal = getattr(error, 'signum', signal.SIGINT)
                        sample['status'] = ('cancelled' if isinstance(error, (KeyboardInterrupt, SystemExit))
                                            else 'invalid-or-incomplete')
                        sample['error'] = {'type': type(error).__name__, 'message': str(error)}
                        raise
                    finally:
                        errors = finish_attempt(sample, (stdout_path, stderr_path, metadata_path, raw_path), record_existing)
                        checkpoint()
                        if errors and sample['status'] == 'completed':
                            sample['status'] = 'invalid-or-incomplete'
                            raise ValueError('Attempt artifacts could not be retained')
                if pair['baseline']['output_sha256'] != pair['candidate']['output_sha256']:
                    report['portable_promotion'] = 'blocked: same-policy byte mismatch'
                    raise ValueError('Same-policy byte mismatch')
                pair['finished_monotonic_ns'] = time.monotonic_ns()
                checkpoint()
            print(f"completed {len(report['cells'])}/{len(CELLS)}: {name}/{mode}/full", flush=True)
        for side, root in roots.items():
            current = source_identity(root)
            if any(current[key] != identities[side][key] for key in ('root', 'commit', 'source_sha256')):
                raise ValueError('Source changed during diagnostic')
            if hash_regular_file(Path(identities[side]['module_path'])) != identities[side]['module_sha256']:
                raise ValueError('Original module changed during diagnostic')
            if hash_regular_file(Path(identities[side]['build_receipt_path'])) != identities[side]['build_receipt_sha256']:
                raise ValueError('Build receipt changed during diagnostic')
        for name, digest in harness.items():
            if hash_regular_file(roots['candidate'] / ('benchmarks/' + name)) != digest:
                raise ValueError('Fixture/helper harness changed during diagnostic')
        for path, digest in ((driver, report['driver']['sha256']), (wrapper, report['wrapper']['sha256']),
                             (node, report['node']['sha256']), (bindings_path, report['native_bindings']['sha256']),
                             *frozen_paths.items()):
            if hash_regular_file(path) != digest:
                raise ValueError('Driver/runtime/binding changed during diagnostic')
        if report['verified_outputs'] != 336:
            raise ValueError('Incomplete output evidence')
        suspended, fallback_review = [], []
        # No interim timing analysis. All empirical summaries follow all samples.
        for cell in report['cells']:
            pairs = [pair for pair in cell['pairs'] if pair['pair'] >= 0]
            values = {side: [pair[side]['stages']['render_ns'] for pair in pairs] for side in roots}
            ratios = [pair['candidate']['stages']['render_ns'] / pair['baseline']['stages']['render_ns'] for pair in pairs]
            median_ratio = statistics.median(values['candidate']) / statistics.median(values['baseline'])
            count_above = sum(ratio > 1.02 for ratio in ratios)
            regression_signal = cell['n16'] and median_ratio > 1.05 and count_above >= 10
            cell['diagnostic'] = {'median_ns': {side: statistics.median(v) for side, v in values.items()},
                'p95_ns': {side: quantile(v, .95) for side, v in values.items()},
                'median_ratio': median_ratio,
                'p95_ratio': quantile(values['candidate'], .95) / quantile(values['baseline'], .95),
                'paired_ratios': ratios, 'paired_ratios_above_1_02': count_above,
                'material_regression_signal': regression_signal}
            cell['linear_memory_end_bytes'] = {side: [pair[side]['linear_memory']['after_start_bytes'] for pair in pairs] for side in roots}
            if regression_signal:
                suspended.append(cell['case'] + '/' + cell['mode'] + '/full')
            elif not cell['n16'] and median_ratio > 1.05 and count_above >= 10:
                fallback_review.append(cell['case'] + '/' + cell['mode'] + '/full')
        report['suspension_cells'] = suspended
        report['fallback_review_cells'] = fallback_review
        report['portable_promotion'] = ('suspended: material WASM regression signal; investigate before native confirmation'
                                        if suspended else 'no diagnostic suspension signal; no speed/admission claim')
        report['status'] = 'completed'
    except BaseException as error:
        if isinstance(error, (KeyboardInterrupt, SystemExit)):
            cancellation_signal = getattr(error, 'signum', signal.SIGINT)
        report['status'] = 'cancelled' if cancellation_signal is not None else 'invalid-or-incomplete'
        report['cancellation_signal'] = cancellation_signal
        report['error'] = {'type': type(error).__name__, 'message': str(error)}
        if report['portable_promotion'] == 'not determined':
            report['portable_promotion'] = 'not cleared: incomplete diagnostic'
        raise
    finally:
        try:
            report['completed_at_unix_ns'] = time.time_ns()
            checkpoint()
            digest = sha(read_bounded_regular(output / 'report.json', MAX_BYTES))
            (output / 'report.json.sha256').write_text(digest + '\n')
        finally:
            for signum, handler in previous_signals.items():
                signal.signal(signum, handler)


if __name__ == '__main__':
    main()
