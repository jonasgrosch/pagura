"""Independent fixed-matrix WASI evidence audit; standard library, never renders."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path, PurePosixPath
import random
import stat
import statistics

LIMIT = 64 * 1024 * 1024
SIDES = ('baseline', 'candidate')
NAMES = ('opaque64', 'sparse64', 'image-only', 'f32-shading-dense',
         'f32-pattern-dense', 'default-knockout64')
CELLS = tuple((name, mode) for name in NAMES for mode in ('exact', 'competitive'))
HELPERS = {'f32_painted_interval.py', 'f32_painted_interval_fixtures.py',
           'n16_painted_interval.py', 'n16_painted_interval_fixtures.py',
           'n16_output_u16.py', 'n16_output_guarded.py', 'n16_output.py',
           'opaque_composition.py', 'paired_render.py', 'glyph_memo.py',
           'glyph_memo_fixtures.py', 'scene_index.py', 'clip_viewport_fixtures.py'}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def read(path, limit=LIMIT):
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | getattr(os, 'O_NOFOLLOW', 0))
    with os.fdopen(fd, 'rb') as handle:
        info = os.fstat(handle.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_size <= limit, 'Not a bounded regular file: ' + str(path))
        data = handle.read(limit + 1)
        require(len(data) <= limit, 'File grew beyond bound')
        return data


def file_sha(path):
    return sha(read(Path(path).resolve(), 256 * 1024 * 1024))


def unique(items):
    result = {}
    for key, value in items:
        require(key not in result, 'Duplicate JSON key: ' + key)
        result[key] = value
    return result


def decode(data):
    return json.loads(data, object_pairs_hook=unique,
                      parse_constant=lambda value: (_ for _ in ()).throw(ValueError('Nonfinite JSON: ' + value)))


def pinned(path, digest):
    require(isinstance(digest, str) and len(digest) == 64 and all(c in '0123456789abcdef' for c in digest), 'Invalid SHA pin')
    data = read(Path(path).resolve())
    require(sha(data) == digest, 'Independent pin mismatch: ' + str(path))
    return decode(data)


def completed(receipt):
    require(receipt.get('status') == 'completed' and receipt.get('source_unchanged') is True
            and receipt.get('source_before') == receipt.get('source_after')
            and not receipt.get('errors') and not receipt.get('artifact_errors')
            and receipt.get('cancellation_signal') is None
            and all(type(receipt.get(key)) is int and receipt[key] == 0
                    for key in ('exit_code', 'wrapper_exit_code')), 'Incomplete or failed outer receipt')


def expected(name):
    """Literal device geometry; no generator, parser or renderer import."""
    require(name in NAMES, 'Unknown independent oracle')
    lattice = name in ('opaque64', 'sparse64', 'default-knockout64')
    side = 2048 if lattice else 512
    gray = 0 if name == 'opaque64' else 191
    rgb = bytearray(b'\xff' * (side * side * 3)) if lattice else bytearray([191]) * (side * side * 3)
    if lattice:
        # PDF glyph rectangles: x=256*c, y=16+256*r, width256,height6.
        # Disjoint black at alpha1 or1/4; quarter alpha gives255*3/4=191.25.
        for row in range(8):
            for y in range(2048 - (16 + row * 256) - 6, 2048 - (16 + row * 256)):
                rgb[y * side * 3:(y + 1) * side * 3] = bytes([gray]) * (side * 3)
    rgb = bytes(rgb)
    rgba = bytearray(b'\xff' * (side * side * 4))
    for channel in range(3):
        rgba[channel::4] = rgb[channel::3]
    return (side, rgb, f'P6\n{side} {side}\n255\n'.encode() + rgb,
            f'RGBA8 {side} {side} premultiplied\n'.encode() + bytes(rgba))


def schedule(profile):
    require(profile in ('simd', 'scalar'), 'Unknown profile')
    rng, cells = random.Random(20261111), []
    for name, mode in CELLS:
        if profile == 'simd':
            measured = [list(SIDES) for _ in range(6)] + [list(reversed(SIDES)) for _ in range(6)]
            rng.shuffle(measured)
            warm = [list(SIDES), list(reversed(SIDES))]
            rng.shuffle(warm)
            orders = warm + measured
        else:
            orders = [list(SIDES)]
        cells.append({'case': name, 'mode': mode, 'operation': 'full', 'orders': orders})
    return cells


def p95(values):
    ordered = sorted(values)
    position = (len(ordered) - 1) * .95
    lo, hi = math.floor(position), math.ceil(position)
    return ordered[lo] + (ordered[hi] - ordered[lo]) * (position - lo)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ('directory', 'receipt', 'guard', 'pins', 'output'):
        parser.add_argument('--' + key, type=Path, required=True)
    for key in ('receipt', 'guard', 'pins'):
        parser.add_argument('--' + key + '-sha256', required=True)
    args = parser.parse_args()
    root, receipt_path, guard_path = args.directory.resolve(), args.receipt.resolve(), args.guard.resolve()
    pins = pinned(args.pins, args.pins_sha256)
    receipt, guard = pinned(receipt_path, args.receipt_sha256), pinned(guard_path, args.guard_sha256)
    completed(receipt)
    profile = pins['runtime_profile']
    cells = schedule(profile)
    warmups, pairs, outputs = (2, 12, 336) if profile == 'simd' else (0, 1, 24)
    require(pins.get('schema') == 1 and pins.get('experiment') == 'f32-painted-interval-wasi-diagnostic', 'Wrong audit pins')
    report_raw = read(root / 'report.json')
    report = decode(report_raw)
    require(receipt['artifacts'].get(str(root / 'report.json')) == sha(report_raw)
            and receipt['artifacts'].get(str(guard_path)) == args.guard_sha256
            and receipt['artifacts'].get(str(root / 'plan.json')) == file_sha(root / 'plan.json'), 'Final receipt does not bind report/guard/plan')
    require(read(root / 'report.json.sha256').decode().strip() == sha(report_raw), 'Report sidecar differs')
    require(report.get('schema') == 1 and report.get('experiment') == pins['experiment']
            and report.get('runtime_profile') == profile and report.get('status') == 'completed'
            and report.get('admission') is False and report.get('verified_outputs') == outputs
            and report.get('portable_promotion') == 'not determined: diagnostic/preflight only; no speed, RSS or admission claim'
            and not report.get('error') and report.get('cancellation_signal') is None, 'Incomplete/wrong diagnostic report')
    require(report['run_id'] == receipt['run_id'] == report['coordination']['run_id']
            and Path(report['coordination']['receipt']).resolve() == receipt_path, 'Run identity mismatch')
    require(guard.get('status') == 'completed' and guard.get('observation_only') is False
            and type(guard.get('child_exit_code')) is int and guard['child_exit_code'] == 0
            and type(guard.get('exit_code')) is int and guard['exit_code'] == 0
            and guard.get('cancellation_requested_utc') is None, 'Strict observer failed')
    events = guard.get('events')
    require(isinstance(events, list) and all(isinstance(event, dict) and event.get('matches') == [] for event in events)
            and any(event.get('stage') == 'preflight' for event in events)
            and any(event.get('stage') == 'postflight' for event in events), 'Incomplete or contaminated observer')

    # All launcher/interpreter/runtime bytes are external prelaunch pins.
    for label in ('python', 'node', 'driver', 'wrapper', 'capture', 'observer'):
        require(file_sha(pins[label]['path']) == pins[label]['sha256'], 'Runtime/helper changed: ' + label)
    runtime_files = pins['runtime_files_sha256']
    require(runtime_files and all(file_sha(path) == digest for path, digest in runtime_files.items()), 'Python runtime changed')
    command = pins['producer_argv']
    require(command[:3] == [pins['python']['path'], '-B', pins['driver']['path']], 'Producer launch prefix differs')
    require(len(command[3:]) % 2 == 0 and all(key.startswith('--') for key in command[3::2]), 'Malformed producer options')
    options = unique(zip(command[3::2], command[4::2]))
    require(guard['command'] == command and receipt['argv'] == [pins['python']['path'], '-B', pins['observer']['path'],
            '--evidence-dir', str(guard_path.parent), '--', *command], 'Receipt/observer/direct producer chain differs')
    require(report['argv'][1:] == command[1:]
            and str(Path(report['argv'][0]).resolve()) in runtime_files
            and str(Path(command[0]).resolve()) in runtime_files
            and report['environment']['python_executable'] in runtime_files
            and report['environment']['python_sha256'] == runtime_files[report['environment']['python_executable']], 'Unpinned Python launcher handoff')

    artifacts, cache = report['artifacts'], {}
    require(sum(entry['bytes'] for entry in artifacts.values()) == report['stored_bytes'] <= 512 * 1024 * 1024, 'Retained byte ledger differs')

    def artifact(name):
        pure = PurePosixPath(name)
        require(not pure.is_absolute() and '..' not in pure.parts and str(pure) == name
                and (root / name).resolve().is_relative_to(root), 'Unsafe artifact path')
        data = read(root / name)
        require(name in artifacts and len(data) == artifacts[name]['bytes'] and sha(data) == artifacts[name]['sha256'], 'Artifact hash differs: ' + name)
        return data

    for name in artifacts:
        data = artifact(name)
        if name.startswith('blobs/'):
            cache[name] = data
    bound = {}
    for label, relative in (('native_bindings', 'native-bindings'), ('frozen_plan', 'plan'), ('build_bindings', 'build-bindings')):
        descriptor = pins[label]
        original = pinned(descriptor['path'], descriptor['sha256'])
        retained = artifact('bindings/' + relative + '.json')
        require(sha(retained) == descriptor['sha256'] and decode(retained) == original, 'Retained external binding differs')
        bound[label] = original
    native, frozen, builds = bound['native_bindings'], bound['frozen_plan'], bound['build_bindings']
    require(native.get('schema') == 1 and native.get('experiment') == 'f32-painted-interval'
            and native.get('stage') == 'preflight-inputs' and native['plan'] == pins['frozen_plan'], 'Wrong prerequisite binding stage/plan')
    require(native['python'] == pins['python'] and native['guard'] == pins['observer'], 'Native/runtime coordinator pins differ')
    require(frozen.get('experiment') == 'f32-painted-interval' and frozen.get('status') == 'frozen-before-pilot', 'Native plan not frozen')
    feature = '+simd128' if profile == 'simd' else '-simd128'
    flags = '-C target-feature=' + feature
    require(builds.get('schema') == 1 and builds.get('target') == 'wasm32-wasip1'
            and builds.get('profile') == 'release' and builds.get('target_feature') == feature
            and builds.get('rustflags') == flags and 'cargo_encoded_rustflags' in builds
            and builds['cargo_encoded_rustflags'] is None, 'Wrong build feature binding')
    expected_options = {'--runtime-profile': profile, '--bindings': pins['native_bindings']['path'],
        '--bindings-sha256': pins['native_bindings']['sha256'], '--build-bindings': pins['build_bindings']['path'],
        '--build-bindings-sha256': pins['build_bindings']['sha256'], '--plan': pins['frozen_plan']['path'],
        '--plan-sha256': pins['frozen_plan']['sha256'], '--node': pins['node']['path'], '--output': str(root)}
    targets = set()
    for side in SIDES:
        identity, build = report['identities'][side], builds[side]
        source_root = Path(identity['root'])
        require(all(identity[key] == native['identities'][side][key] == frozen['identities_to_freeze'][side][key]
                    for key in ('root', 'commit', 'source_sha256'))
                and build['source_root'] == str(source_root) and build['source'] == native['runner_sources'][side]
                and build['source']['commit'] == identity['commit'], 'Source identity mismatch')
        for option, value in (('source', str(source_root)), ('wasm', build['path']), ('build-receipt', build['receipt'])):
            expected_options['--' + side + '-' + option] = value
        module_sha = sha(artifact(f'bindings/{side}.wasm'))
        require(module_sha == build['sha256'] == identity['module_sha256'] == file_sha(build['path'])
                and identity['module_path'] == build['path'], 'Actual module binding differs')
        build_raw = artifact(f'bindings/{side}-build-receipt.json')
        require(sha(build_raw) == build['receipt_sha256'] == file_sha(build['receipt']) == identity['build_receipt_sha256']
                and identity['build_receipt_path'] == build['receipt'], 'Build receipt pin differs')
        build_receipt = decode(build_raw)
        completed(build_receipt)
        require(build_receipt['source_root'] == str(source_root) and build_receipt['cwd'] == str(source_root)
                and build_receipt['source_before'] == build['source']
                and build_receipt['artifacts'].get(build['path']) == module_sha, 'Build source/module mismatch')
        context_raw = artifact(f'bindings/{side}-build-context.json')
        context_path, context_sha = build['context']['path'], build['context']['sha256']
        require(sha(context_raw) == context_sha == file_sha(context_path)
                and build_receipt['artifacts'].get(context_path) == context_sha
                and identity['build_context_path'] == context_path and identity['build_context_sha256'] == context_sha, 'Unbound actual build context')
        context = decode(context_raw)
        target = Path(context['CARGO_TARGET_DIR'])
        require(target.is_absolute() and target == target.resolve() and str(target) not in targets
                and Path(build['path']).is_relative_to(target / 'wasm32-wasip1/release/examples'), 'Shared or incorrect build target')
        targets.add(str(target))
        cargo_command = ['/usr/bin/env', '-u', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTFLAGS=' + flags,
            context['cargo']['path'], 'build', '--locked', '--offline', '--release', '-p', 'pagura-render',
            '--example', 'scene_index_profile', '--target', 'wasm32-wasip1', '--target-dir', str(target),
            '-j', '1', '--message-format=json-render-diagnostics']
        capture_suffix = ['-B', pins['capture']['path'], context_path, '--', *cargo_command]
        require(context['schema'] == 1 and context['root'] == str(source_root) and context['command'] == cargo_command
                and context['RUSTFLAGS'] == flags and context['CARGO_ENCODED_RUSTFLAGS'] is None
                and context['CARGO_BUILD_JOBS'] == '1' and context['capture_sha256'] == pins['capture']['sha256']
                and context['capture_argv'][1:] == capture_suffix
                and str(Path(context['capture_argv'][0]).resolve()) in runtime_files
                and build_receipt['argv'][-len(capture_suffix)-1:] == [pins['python']['path'], *capture_suffix]
                and file_sha(context['cargo']['path']) == context['cargo']['sha256'], 'Actual build argv/environment differs')
        config = source_root / '.cargo/config.toml'
        require(context['cargo_config'] == {'path': str(config), 'sha256': file_sha(config)}, 'Build Cargo config changed')
    require(options == expected_options, 'Actual producer options differ from pinned inputs')
    require(receipt['source_root'] == report['identities']['candidate']['root']
            and receipt['source_before'] == native['runner_sources']['candidate'], 'Outer source identity differs')
    require(set(report['harness_sha256']) == HELPERS, 'Missing imported helper closure')
    candidate = Path(report['identities']['candidate']['root'])
    for name, digest in report['harness_sha256'].items():
        require(native['bound_files_sha256'].get('benchmarks/' + name) == digest
                and file_sha(candidate / 'benchmarks' / name) == digest, 'Fixture/helper binding changed')
    for label, relative in (('driver', 'diagnostic.py'), ('wrapper', 'run_profile_wasi.mjs')):
        require(sha(artifact('bindings/' + relative)) == pins[label]['sha256'] == report[label]['sha256']
                and report[label]['path'] == pins[label]['path']
                and native['bound_files_sha256'].get(str(Path(pins[label]['path']).relative_to(candidate))) == pins[label]['sha256'], 'Unbound WASI code')
    require(report['node']['sha256'] == pins['node']['sha256'] and report['node']['path'] == pins['node']['path'], 'Node binding differs')
    require(report['native_bindings'] == pins['native_bindings'] and report['frozen_plan_sha256'] == pins['frozen_plan']['sha256']
            and report['build_bindings_sha256'] == pins['build_bindings']['sha256'], 'Report binding descriptors differ')
    example = 'crates/pagura-render/examples/scene_index_profile.rs'
    require(report['example_sha256'] == native['bound_files_sha256'][example]
            and all(file_sha(Path(report['identities'][side]['root']) / example) == report['example_sha256'] for side in SIDES), 'Profile example differs')
    plan_raw = artifact('plan.json')
    plan = decode(plan_raw)
    require(sha(plan_raw) == report['plan_sha256'] and plan['cells'] == cells
            and all(plan.get(key) == value for key, value in {'schema': 1, 'seed': 20261111,
                'profile': profile, 'warmups': warmups, 'pairs': pairs, 'outputs': outputs, 'dpi': 72,
                'metric': 'render_ns' if profile == 'simd' else None}.items()), 'Fixed plan differs')
    oracles = {name: expected(name) for name in NAMES}
    require(set(pins['inputs_sha256']) == set(NAMES), 'Missing independent input pins')
    for name, (_, _, ppm, _) in oracles.items():
        require(artifact(f'oracles/{name}.ppm') == ppm and sha(artifact(f'inputs/{name}.pdf')) == pins['inputs_sha256'][name], 'Frozen input or independent oracle differs')
    require(len(report['cells']) == len(cells), 'Missing/extra cell')
    checked, summaries, last_finished = 0, [], 0
    for cell, spec in zip(report['cells'], cells):
        name, mode = spec['case'], spec['mode']
        require(all(cell.get(key) == spec[key] for key in ('case', 'mode', 'operation'))
                and len(cell['pairs']) == warmups + pairs, 'Cell matrix differs')
        size, rgb, _, raw = oracles[name]
        for index, (pair, order) in enumerate(zip(cell['pairs'], spec['orders'])):
            require(pair['pair'] == index - warmups and pair['order'] == order, 'Pair schedule differs')
            require(type(pair['started_monotonic_ns']) is int and type(pair['finished_monotonic_ns']) is int
                    and pair['finished_monotonic_ns'] >= pair['started_monotonic_ns'] >= last_finished, 'Incomplete or reordered pair interval')
            previous_arm = pair['started_monotonic_ns']
            for side in order:
                sample = pair[side]
                relative = f'samples/{name}-{mode}-{index:02d}-{side}'
                directory = root / relative
                invocation = [pins['node']['path'], pins['wrapper']['path'], str(root / f'bindings/{side}.wasm'),
                    str(root / f'inputs/{name}.pdf'), mode, str(directory / 'render.rgba'),
                    str(directory / 'wasi.stdout'), str(directory / 'wasi.stderr'), str(directory / 'wrapper.json')]
                require(sample['status'] == 'completed' and not sample.get('error') and not sample.get('artifact_errors')
                        and sample['command'] == invocation and type(sample['process']['exit_code']) is int
                        and sample['process']['exit_code'] == 0 and sample['process']['status'] != 'timeout', 'Failed or substituted invocation')
                require(type(sample['started_monotonic_ns']) is int and type(sample['finished_monotonic_ns']) is int
                        and previous_arm <= sample['started_monotonic_ns'] <= sample['finished_monotonic_ns']
                        <= pair['finished_monotonic_ns'], 'Incomplete or reordered arm interval')
                previous_arm = sample['finished_monotonic_ns']
                require(artifact(relative + '/node.log').decode() == sample['diagnostic'], 'Raw Node log differs')
                metadata = decode(artifact(relative + '/wrapper.json'))
                require(metadata['status'] == 'completed' and type(metadata['exit_code']) is int and metadata['exit_code'] == 0
                        and metadata['argv'] == invocation and metadata['module_sha256'] == builds[side]['sha256']
                        and metadata['input_sha256'] == pins['inputs_sha256'][name] and metadata['wrapper_sha256'] == pins['wrapper']['sha256']
                        and metadata['runtime'] == report['node']['runtime'] and metadata['runtime']['exec_argv'] == []
                        and metadata['runtime']['node_options'] == '' and Path(metadata['runtime']['executable']).resolve() == Path(pins['node']['path']).resolve(), 'WASI runtime/request binding differs')
                require(metadata['wasi_args'] == ['scene_index_profile', f'/input/{name}.pdf', mode, 'full', '/output/render.rgba', '72']
                        and metadata['wasi_env'] == {} and metadata['preopens'] == {'/input': str(root / 'inputs'), '/output': str(directory)}, 'WASI args/preopens differ')
                memory = metadata['linear_memory']
                before, after = memory['before_start_bytes'], memory['after_start_bytes']
                require((before is None and after is None) or (type(before) is int and type(after) is int
                        and 0 < before <= after and before % 65536 == after % 65536 == 0), 'Invalid linear-memory metadata')
                require(sample['linear_memory'] == memory and artifact(relative + '/wasi.stderr') == b'', 'Unexpected program diagnostics')
                stages = decode(artifact(relative + '/wasi.stdout'))
                require(stages == sample['stages'] and all(type(stages.get(key)) is int and stages[key] > 0
                        for key in ('open_ns', 'compile_ns', 'render_ns', 'total_ns', 'metric_ns'))
                        and stages['schema'] == 1 and stages['operation'] == 'full' and stages['accuracy'] == mode
                        and stages['dpi'] == 72 and stages['width'] == stages['height'] == size
                        and stages['metric_ns'] == stages['render_ns']
                        and stages['total_ns'] >= stages['open_ns'] + stages['compile_ns'] + stages['render_ns'], 'Actual stage/request mismatch')
                blob = sample['output_blob']
                require(blob == 'blobs/' + sha(raw) + '.rgba' and cache.get(blob) == raw and sample['output_sha256'] == sha(raw)
                        and sample['pixels_sha256'] == sha(rgb) and sample['oracle_equal'] is True
                        and sample['width'] == sample['height'] == size, 'Independent raw RGB/opaque-alpha mismatch')
                checked += 1
            require(pair['baseline']['output_sha256'] == pair['candidate']['output_sha256'], 'Same-mode arm bytes differ')
            last_finished = pair['finished_monotonic_ns']
        if profile == 'scalar':
            require('diagnostic' not in cell and 'linear_memory_end_bytes' not in cell, 'Scalar preflight acquired a timing summary')
            continue
        measured = cell['pairs'][warmups:]
        values = {side: [pair[side]['stages']['render_ns'] for pair in measured] for side in SIDES}
        result = {'median_ns': {side: statistics.median(values[side]) for side in SIDES},
            'p95_ns': {side: p95(values[side]) for side in SIDES},
            'median_ratio': statistics.median(values['candidate']) / statistics.median(values['baseline']),
            'p95_ratio': p95(values['candidate']) / p95(values['baseline']),
            'paired_ratios': [pair['candidate']['stages']['render_ns'] / pair['baseline']['stages']['render_ns'] for pair in measured]}
        require(cell['diagnostic'] == result and cell['linear_memory_end_bytes'] == {
            side: [pair[side]['linear_memory']['after_start_bytes'] for pair in measured] for side in SIDES}, 'Descriptive summary differs')
        summaries.append({'case': name, 'mode': mode, **result})
    require(checked == outputs, 'Missing raw output evidence')
    result = {'schema': 1, 'status': 'verified', 'experiment': pins['experiment'], 'runtime_profile': profile,
        'verified_outputs': checked, 'receipt_sha256': args.receipt_sha256, 'report_sha256': sha(report_raw),
        'guard_sha256': args.guard_sha256, 'native_bindings_sha256': pins['native_bindings']['sha256'],
        'frozen_plan_sha256': pins['frozen_plan']['sha256'], 'build_bindings_sha256': pins['build_bindings']['sha256'],
        'pins_sha256': args.pins_sha256, 'auditor_sha256': file_sha(__file__), 'background_contention_observed': False,
        'independent_analytic_pixels': True, 'admission': False, 'cells': summaries,
        'limitations': ['Descriptive SIMD diagnostics only; scalar timings are not summarized.',
            'No speed/RSS admission or global portability claim; independent fixture construction and fresh Cargo selection remain reviewed build evidence.',
            'Strict observer cannot prove absence of every short or unrecognized workload.']}
    data = (json.dumps(result, indent=2) + '\n').encode()
    with args.output.resolve().open('xb') as handle:
        handle.write(data)
    print(json.dumps({'status': 'verified', 'runtime_profile': profile, 'verified_outputs': checked, 'sha256': sha(data)}))


if __name__ == '__main__':
    main()
