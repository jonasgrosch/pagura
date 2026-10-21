#!/usr/bin/env python3
"""Independent retained-evidence audit; standard library only, no renderer imports.

All --*-sha256 values are independent coordinator pins, not values copied from
the report under audit. The completed outer receipt authenticates report bytes.
Run only after every sample/observer/receipt has finished. Never renders anything.
"""
import argparse
import hashlib
import json
import math
import os
import random
import stat
import statistics
from pathlib import Path, PurePosixPath

CELLS = (('opaque64', 'exact'), ('opaque64', 'competitive'),
         ('opaque-fill', 'exact'), ('opaque-fill', 'competitive'),
         ('default-knockout64', 'exact'), ('image-only', 'exact'))
SIDES = ('baseline', 'candidate')
HARNESS = {'n16_output_u16.py', 'n16_output_guarded.py', 'n16_output.py',
           'opaque_composition.py', 'paired_render.py', 'glyph_memo.py',
           'glyph_memo_fixtures.py', 'scene_index.py', 'clip_viewport_fixtures.py'}
BASELINE = 'f98ad8365b46d500cbb866b13fcea0b0cbb28f7f'
LIMIT = 64 * 1024 * 1024


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def read(path, limit=LIMIT):
    descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK | getattr(os, 'O_NOFOLLOW', 0))
    with os.fdopen(descriptor, 'rb') as handle:
        metadata = os.fstat(handle.fileno())
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_size <= limit, 'Not a bounded regular file: ' + str(path))
        data = handle.read(limit + 1)
        require(len(data) <= limit, 'File grew beyond bound: ' + str(path))
        return data


def file_hash(path, limit=1024 * 1024 * 1024):
    descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK | getattr(os, 'O_NOFOLLOW', 0))
    with os.fdopen(descriptor, 'rb') as handle:
        metadata = os.fstat(handle.fileno())
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_size <= limit, 'Unbounded identity file')
        digest, size = hashlib.sha256(), 0
        while chunk := handle.read(1024 * 1024):
            size += len(chunk)
            require(size <= limit, 'Identity file grew beyond bound')
            digest.update(chunk)
        return digest.hexdigest()


def unique_pairs(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, 'Duplicate JSON key: ' + key)
        result[key] = value
    return result


def decode(data):
    return json.loads(data, object_pairs_hook=unique_pairs,
                      parse_constant=lambda value: (_ for _ in ()).throw(ValueError('Nonfinite JSON: ' + value)))


def pinned(path, digest):
    require(len(digest) == 64 and all(c in '0123456789abcdef' for c in digest), 'Invalid independent SHA-256')
    data = read(path)
    require(sha(data) == digest, 'Independent pin mismatch: ' + str(path))
    return decode(data)


def success(receipt):
    require(receipt.get('status') == 'completed' and receipt.get('source_unchanged') is True
            and receipt.get('source_before') == receipt.get('source_after')
            and not receipt.get('errors') and not receipt.get('artifact_errors')
            and receipt.get('cancellation_signal') is None
            and all(type(receipt.get(key)) is int and receipt[key] == 0
                    for key in ('exit_code', 'wrapper_exit_code')), 'Incomplete/failed runner receipt')


def quantile(values):
    ordered = sorted(values)
    position = (len(ordered) - 1) * .95
    lo, hi = math.floor(position), math.ceil(position)
    return ordered[lo] + (ordered[hi] - ordered[lo]) * (position - lo)


def expected_pixels(name):
    """Independent integer geometry, not producer/renderer fixture code.

    At 72 dpi, 64 nonoverlapping 256x6 glyph rectangles start at PDF y=16
    in an 8x8 lattice with step256. Quarter-alpha black over white rounds
    to191. The two 512-square fixtures are uniformly black or gray191.
    This trusts the frozen generator's PDF construction, not arbitrary PDFs.
    """
    size = 2048 if name in ('opaque64', 'default-knockout64') else 512
    if name in ('opaque64', 'default-knockout64'):
        rgb = bytearray(b'\xff' * (size * size * 3))
        gray = 0 if name == 'opaque64' else 191
        stripe = bytes([gray]) * (256 * 3)
        for index in range(64):
            x, y = index % 8 * 256, 16 + index // 8 * 256
            for pdf_y in range(y, y + 6):
                start = ((size - 1 - pdf_y) * size + x) * 3
                rgb[start:start + len(stripe)] = stripe
        rgb = bytes(rgb)
    else:
        require(name in ('opaque-fill', 'image-only'), 'Unknown oracle case')
        rgb = bytes([0 if name == 'opaque-fill' else 191]) * (size * size * 3)
    rgba = bytearray(b'\xff' * (size * size * 4))
    for channel in range(3):
        rgba[channel::4] = rgb[channel::3]
    ppm = f'P6\n{size} {size}\n255\n'.encode() + rgb
    raw = f'RGBA8 {size} {size} premultiplied\n'.encode() + bytes(rgba)
    return size, rgb, ppm, raw


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('directory', 'receipt', 'guard', 'build-bindings', 'output'):
        parser.add_argument('--' + name, required=True, type=Path)
    for name in ('receipt', 'guard', 'build-bindings', 'native-bindings', 'driver', 'wrapper', 'node'):
        parser.add_argument('--' + name + '-sha256', required=True)
    args = parser.parse_args()
    root, receipt_path, guard_path = args.directory.resolve(), args.receipt.resolve(), args.guard.resolve()
    receipt = pinned(receipt_path, args.receipt_sha256)
    guard = pinned(guard_path, args.guard_sha256)
    builds = pinned(args.build_bindings.resolve(), args.build_bindings_sha256)
    success(receipt)
    report_raw = read(root / 'report.json')
    report_hash = sha(report_raw)
    require(receipt['artifacts'].get(str(root / 'report.json')) == report_hash, 'Outer receipt does not pin report')
    require(receipt['artifacts'].get(str(guard_path)) == args.guard_sha256, 'Outer receipt does not pin observer')
    require(read(root / 'report.json.sha256').decode().strip() == report_hash, 'Report hash sidecar mismatch')
    report = decode(report_raw)
    require(report['schema'] == 1 and report['experiment'] == 'n16-output-u16-wasi-diagnostic'
            and report['status'] == 'completed' and report['admission'] is False
            and report['verified_outputs'] == 168, 'Wrong/incomplete diagnostic')
    require(report['run_id'] == receipt['run_id'] == report['coordination']['run_id']
            and Path(report['coordination']['receipt']).resolve() == receipt_path, 'Run/receipt identity mismatch')
    require(guard.get('status') == 'completed-observation-only' and guard.get('observation_only') is True
            and type(guard.get('child_exit_code')) is int and guard['child_exit_code'] == 0
            and type(guard.get('exit_code')) is int and guard['exit_code'] == 0, 'Observer did not finish')
    events = guard.get('events')
    require(isinstance(events, list) and any(e.get('stage') == 'preflight' for e in events)
            and any(e.get('stage') == 'postflight' for e in events), 'Missing observer coverage')
    contention = any(event.get('matches') for event in events)

    artifacts, cache = report['artifacts'], {}
    require(sum(item['bytes'] for item in artifacts.values()) == report['stored_bytes'] <= 512 * 1024 * 1024,
            'Retention byte accounting mismatch')

    def artifact(name):
        pure = PurePosixPath(name)
        require(not pure.is_absolute() and '..' not in pure.parts and str(pure) == name, 'Unsafe artifact path')
        path = root / name
        require(path.resolve().is_relative_to(root), 'Artifact escapes directory')
        require(name in artifacts, 'Missing retained artifact: ' + name)
        data = read(path)
        require(len(data) == artifacts[name]['bytes'] and sha(data) == artifacts[name]['sha256'],
                'Artifact bytes/hash mismatch: ' + name)
        return data

    # Read every retained artifact, including all warmup logs and unused bindings.
    for name in artifacts:
        data = artifact(name)
        if name.startswith('blobs/'):
            cache[name] = data
    native_raw = artifact('bindings/native-bindings.json')
    require(sha(native_raw) == args.native_bindings_sha256 == report['native_bindings']['sha256'],
            'Native binding pin mismatch')
    native = decode(native_raw)
    for label, digest in (('driver', args.driver_sha256), ('wrapper', args.wrapper_sha256)):
        name = 'diagnostic.py' if label == 'driver' else 'run_profile_wasi.mjs'
        require(sha(artifact('bindings/' + name)) == digest == report[label]['sha256'], 'Frozen diagnostic script mismatch')
    require(report['node']['sha256'] == args.node_sha256
            and file_hash(Path(report['node']['path'])) == args.node_sha256, 'Node executable pin mismatch')
    require(receipt['source_root'] == report['identities']['candidate']['root']
            and receipt['source_before'] == builds['candidate']['source'], 'Outer runner source mismatch')
    cargo_argv = []
    for side in SIDES:
        identity, build = report['identities'][side], builds[side]
        require(identity['commit'] == BASELINE and identity['root'] == build['source_root']
                and all(identity[key] == native['identities'][side][key]
                        for key in ('root', 'commit', 'source_sha256')), 'Source identity mismatch')
        module_hash = sha(artifact(f'bindings/{side}.wasm'))
        require(module_hash == build['sha256'] == identity['module_sha256'], 'WASM module pin mismatch')
        build_raw = artifact(f'bindings/{side}-build-receipt.json')
        require(sha(build_raw) == build['receipt_sha256'] == identity['build_receipt_sha256'], 'Build receipt pin mismatch')
        build_receipt = decode(build_raw)
        success(build_receipt)
        require(build_receipt['source_root'] == build['source_root'] and build_receipt['source_before'] == build['source']
                and module_hash in build_receipt['artifacts'].values(), 'Build/source/module correspondence missing')
        argv = build_receipt['argv']
        require('cargo' in argv, 'No recorded Cargo build command')
        cargo_argv.append(argv[argv.index('cargo'):])
    require(cargo_argv[0] == cargo_argv[1] == ['cargo', 'build', '--locked', '--offline', '--release',
            '-p', 'pagura-render', '--example', 'scene_index_profile', '--target', 'wasm32-wasip1', '-j', '1'],
            'Build commands differ from the frozen default WASI profile')
    require(set(report['harness_sha256']) == HARNESS, 'Missing/extra imported helper bindings')
    for name, digest in report['harness_sha256'].items():
        require(native['bound_files_sha256'].get('benchmarks/' + name) == digest, 'Imported helper binding mismatch')
    require(report['example_sha256'] == native['bound_files_sha256']['crates/pagura-render/examples/scene_index_profile.rs'],
            'Example binding mismatch')

    plan_raw = artifact('plan.json')
    require(sha(plan_raw) == report['plan_sha256'], 'Plan pin mismatch')
    plan = decode(plan_raw)
    require(all(plan.get(key) == value for key, value in {'schema': 1, 'seed': 20261103, 'pairs': 12,
            'warmups': 2, 'dpi': 72, 'outputs': 168, 'metric': 'render_ns'}.items()), 'Wrong fixed plan')
    require(plan['triage']['pixel_mismatch'] == 'block portable candidate'
            and plan['triage']['n16_material_regression'] == {'median_ratio_gt': 1.05,
                'paired_ratio_gt': 1.02, 'minimum_pairs_above': 10,
                'action': 'suspend portable promotion; investigate or revise before native confirmation'}, 'Triage rule changed')
    rng, expected_cells = random.Random(20261103), []
    for name, mode in CELLS:
        measured = [['baseline', 'candidate'] for _ in range(6)] + [['candidate', 'baseline'] for _ in range(6)]
        rng.shuffle(measured)
        warm = [['baseline', 'candidate'], ['candidate', 'baseline']]
        rng.shuffle(warm)
        expected_cells.append({'case': name, 'mode': mode, 'operation': 'full',
                               'n16': name in ('opaque64', 'opaque-fill'), 'orders': warm + measured})
    require(plan['cells'] == expected_cells and len(report['cells']) == 6, 'Cell order/pairing schedule changed')
    expected = {name: expected_pixels(name) for name in dict.fromkeys(name for name, _ in CELLS)}
    for name, (_, _, ppm, _) in expected.items():
        require(artifact(f'oracles/{name}.ppm') == ppm, 'Independent analytic oracle mismatch: ' + name)
        require(artifact(f'inputs/{name}.pdf').startswith(b'%PDF-'), 'Missing retained PDF')
    results, checked = [], 0
    for cell, specification in zip(report['cells'], expected_cells):
        name, mode = specification['case'], specification['mode']
        require(all(cell.get(key) == specification[key] for key in ('case', 'mode', 'operation', 'n16'))
                and len(cell['pairs']) == 14, 'Cell identity/count mismatch')
        size, rgb, _, expected_raw = expected[name]
        for index, (pair, order) in enumerate(zip(cell['pairs'], specification['orders'])):
            require(pair['pair'] == index - 2 and pair['order'] == order, 'Pair ID/order mismatch')
            require(type(pair['started_monotonic_ns']) is int and type(pair['finished_monotonic_ns']) is int
                    and pair['finished_monotonic_ns'] >= pair['started_monotonic_ns'], 'Invalid pair interval')
            for side in order:
                sample = pair[side]
                relative = f'samples/{name}-{mode}-{index:02d}-{side}'
                sample_dir = root / relative
                command = [report['node']['path'], report['wrapper']['path'], str(root / f'bindings/{side}.wasm'),
                           str(root / f'inputs/{name}.pdf'), mode, str(sample_dir / 'render.rgba'),
                           str(sample_dir / 'wasi.stdout'), str(sample_dir / 'wasi.stderr'), str(sample_dir / 'wrapper.json')]
                require(sample['command'] == command and sample['process']['exit_code'] == 0
                        and sample['process']['status'] != 'timeout', 'Invocation command/result mismatch')
                require(artifact(relative + '/node.log').decode() == sample['diagnostic'], 'Node log mismatch')
                metadata = decode(artifact(relative + '/wrapper.json'))
                require(metadata['status'] == 'completed' and metadata['exit_code'] == 0
                        and metadata['module_sha256'] == builds[side]['sha256']
                        and metadata['wrapper_sha256'] == args.wrapper_sha256
                        and metadata['input_sha256'] == artifacts[f'inputs/{name}.pdf']['sha256']
                        and metadata['runtime'] == report['node']['runtime']
                        and metadata['runtime']['exec_argv'] == [] and metadata['runtime']['node_options'] == ''
                        and Path(metadata['runtime']['executable']).resolve() == Path(report['node']['path']).resolve()
                        and metadata['argv'] == command, 'WASI runtime/module/request binding mismatch')
                require(metadata['wasi_args'] == ['scene_index_profile', f'/input/{name}.pdf', mode, 'full', '/output/render.rgba', '72']
                        and metadata['wasi_env'] == {} and metadata['preopens'] == {'/input': str(root / 'inputs'), '/output': str(sample_dir)},
                        'Actual WASI arguments/filesystem mapping mismatch')
                memory = metadata['linear_memory']
                before, after = memory['before_start_bytes'], memory['after_start_bytes']
                require((before is None and after is None) or (type(before) is int and type(after) is int
                        and 0 < before <= after and before % 65536 == after % 65536 == 0), 'Invalid linear memory')
                require(sample['linear_memory'] == memory and artifact(relative + '/wasi.stderr') == b'', 'Memory/diagnostic mismatch')
                stages = decode(artifact(relative + '/wasi.stdout'))
                require(stages == sample['stages'] and all(type(stages.get(key)) is int and stages[key] > 0
                        for key in ('open_ns', 'compile_ns', 'render_ns', 'total_ns', 'metric_ns'))
                        and stages['schema'] == 1 and stages['operation'] == 'full' and stages['accuracy'] == mode
                        and stages['dpi'] == 72 and stages['width'] == stages['height'] == size
                        and stages['metric_ns'] == stages['render_ns']
                        and stages['total_ns'] >= stages['open_ns'] + stages['compile_ns'] + stages['render_ns'], 'Stage identity/timing mismatch')
                blob = sample['output_blob']
                require(blob == 'blobs/' + sample['output_sha256'] + '.rgba' and cache.get(blob) == expected_raw
                        and sample['output_sha256'] == sha(expected_raw) and sample['pixels_sha256'] == sha(rgb)
                        and sample['width'] == sample['height'] == size and sample['oracle_equal'] is True, 'Independent raw RGBA oracle mismatch')
                checked += 1
            require(pair['baseline']['output_sha256'] == pair['candidate']['output_sha256'], 'Paired output mismatch')
        pairs = cell['pairs'][2:]
        values = {side: [pair[side]['stages']['render_ns'] for pair in pairs] for side in SIDES}
        ratios = [pair['candidate']['stages']['render_ns'] / pair['baseline']['stages']['render_ns'] for pair in pairs]
        median_ratio = statistics.median(values['candidate']) / statistics.median(values['baseline'])
        count = sum(value > 1.02 for value in ratios)
        result = {'median_ns': {side: statistics.median(values[side]) for side in SIDES},
                  'p95_ns': {side: quantile(values[side]) for side in SIDES}, 'median_ratio': median_ratio,
                  'p95_ratio': quantile(values['candidate']) / quantile(values['baseline']), 'paired_ratios': ratios,
                  'paired_ratios_above_1_02': count,
                  'material_regression_signal': specification['n16'] and median_ratio > 1.05 and count >= 10}
        require(result == cell['diagnostic'], 'Diagnostic summaries/triage do not reproduce')
        memory_end = {side: [pair[side]['linear_memory']['after_start_bytes'] for pair in pairs] for side in SIDES}
        require(memory_end == cell['linear_memory_end_bytes'], 'Linear-memory summary mismatch')
        results.append({'case': name, 'mode': mode, 'operation': 'full', **result, 'linear_memory_end_bytes': memory_end})
    require(checked == 168, 'Incomplete raw sample evidence')
    suspended = [f"{cell['case']}/{cell['mode']}/full" for cell in results if cell['material_regression_signal']]
    expected_decision = ('suspended: material WASM regression signal; investigate before native confirmation' if suspended
                         else 'no diagnostic suspension signal; no speed/admission claim')
    require(report['suspension_cells'] == suspended and report['portable_promotion'] == expected_decision, 'Triage decision mismatch')
    result = {'schema': 1, 'status': 'verified', 'experiment': report['experiment'], 'run_id': report['run_id'],
              'admission': False, 'report_sha256': report_hash, 'receipt_sha256': args.receipt_sha256,
              'guard_sha256': args.guard_sha256, 'plan_sha256': sha(plan_raw),
              'build_bindings_sha256': args.build_bindings_sha256, 'native_bindings_sha256': args.native_bindings_sha256,
              'auditor_sha256': file_hash(Path(__file__).resolve()), 'verified_outputs': checked,
              'rehashed_artifacts': len(artifacts), 'independent_analytic_pixels': True,
              'background_contention_observed': contention, 'suspension_cells': suspended,
              'decision': 'not cleared: observed contention' if contention else expected_decision,
              'unavailable_claims': ['No statistical admission or calibrated confidence inference from12pairs.',
                 'No Node startup, warmed-browser, total-memory or RSS comparison.',
                 'Observer cannot prove absence of short/unrecognized background workloads.',
                 'PDF construction and unrecorded compiler environment remain trusted coordinator/generator evidence.'],
              'cells': results}
    data = (json.dumps(result, indent=2) + '\n').encode()
    with args.output.resolve().open('xb') as handle:
        handle.write(data)
    print(json.dumps({'status': result['status'], 'decision': result['decision'], 'verified_outputs': checked,
                      'audit_sha256': sha(data), 'output': str(args.output.resolve())}))


if __name__ == '__main__':
    main()
