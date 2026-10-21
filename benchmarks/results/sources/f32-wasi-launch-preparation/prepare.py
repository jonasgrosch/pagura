"""Prepare fixed WASI argv and external audit pins; never launch or import tools."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
import sys

NAME = 'f32-painted-interval'
CASES = ('opaque64', 'sparse64', 'image-only', 'f32-shading-dense',
         'f32-pattern-dense', 'default-knockout64')
SIDES = ('baseline', 'candidate')


def require(value, message):
    if not value:
        raise ValueError(message)


def unique(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, 'Duplicate key: ' + str(key))
        result[key] = value
    return result


def read(path):
    fd = os.open(Path(path).resolve(strict=True), os.O_RDONLY | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as source:
        info = os.fstat(source.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_size <= 256 * 1024 * 1024,
                'Not a bounded regular file: ' + str(path))
        data = source.read(256 * 1024 * 1024 + 1)
        require(len(data) <= 256 * 1024 * 1024, 'Input grew beyond bound')
        return data


def digest(path):
    return hashlib.sha256(read(path)).hexdigest()


def descriptor(path):
    return {'path': str(Path(path).resolve(strict=True)), 'sha256': digest(path)}


def load(pin):
    require(descriptor(pin['path']) == pin, 'Independent path/hash pin differs: ' + pin['path'])
    return json.loads(read(pin['path']), object_pairs_hook=unique,
                      parse_constant=lambda value: (_ for _ in ()).throw(ValueError(value)))


def complete(receipt):
    require(receipt.get('status') == 'completed' and receipt.get('source_unchanged') is True
            and receipt.get('source_before') == receipt.get('source_after')
            and not receipt.get('errors') and not receipt.get('artifact_errors')
            and receipt.get('cancellation_signal') is None
            and all(type(receipt.get(k)) is int and receipt[k] == 0
                    for k in ('exit_code', 'wrapper_exit_code')), 'Incomplete final receipt')


def strict(guard):
    require(guard.get('status') == 'completed' and guard.get('observation_only') is False
            and guard.get('cancellation_requested_utc') is None
            and all(type(guard.get(k)) is int and guard[k] == 0 for k in ('exit_code', 'child_exit_code'))
            and isinstance(guard.get('events'), list)
            and all(event.get('matches') == [] for event in guard['events'])
            and {'preflight', 'postflight'} <= {event.get('stage') for event in guard['events']},
            'Incomplete or contaminated strict observer')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    pinned_names = ('bindings', 'selection', 'preflight-report', 'preflight-receipt',
                    'preflight-guard', 'simd-build-bindings', 'scalar-build-bindings', 'node')
    for name in pinned_names:
        parser.add_argument('--' + name, type=Path, required=True)
        parser.add_argument('--' + name + '-sha256', required=True)
    for name in ('output', 'simd-output', 'scalar-output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    pins = {name: {'path': str(getattr(args, name.replace('-', '_')).resolve(strict=True)),
                   'sha256': getattr(args, name.replace('-', '_') + '_sha256')} for name in pinned_names}
    native, selection = load(pins['bindings']), load(pins['selection'])
    report, receipt, guard = (load(pins['preflight-' + name]) for name in ('report', 'receipt', 'guard'))
    require(native['schema'] == 1 and native['experiment'] == NAME and native['stage'] == 'preflight-inputs',
            'Wrong immutable prerequisite binding')
    plan = load(native['plan'])
    root = Path(native['identities']['candidate']['root']).resolve(strict=True)
    require(plan['experiment'] == NAME and plan['status'] == 'frozen-before-pilot'
            and len(plan['cells']) == 56 and plan['identities_to_freeze'] == native['identities'], 'Wrong frozen plan')
    complete(receipt)
    strict(guard)
    require(receipt['artifacts'].get(pins['preflight-report']['path']) == pins['preflight-report']['sha256']
            and receipt['artifacts'].get(pins['preflight-guard']['path']) == pins['preflight-guard']['sha256']
            and receipt['run_id'] == report['run_id']
            and str(Path(report['run_receipt']).resolve()) == pins['preflight-receipt']['path']
            and receipt['source_before'] == native['runner_sources']['candidate'], 'Unbound native preflight')
    require(report['status'] == 'verified-native-preflight' and report['all_correct'] is True
            and report['identities_unchanged'] is True and report['expected_outputs'] == report['completed_outputs'] == 112
            and report['bindings_sha256'] == pins['bindings']['sha256'] and report['identities'] == native['identities'],
            'Native112 did not complete under this frozen input core')
    matrix = [(i, cell['case'], cell['mode'], cell['operation'], side)
              for i, cell in enumerate(plan['cells']) for side in SIDES]
    require([(r['cell'], r['case'], r['mode'], r['operation'], r['side']) for r in report['records']] == matrix
            and all(r.get('verified') is True for r in report['records']), 'Native112 record matrix differs')
    files = {str((root / rel).resolve()): value for rel, value in native['bound_files_sha256'].items()}
    require(all(Path(path).is_relative_to(root) and digest(path) == value for path, value in files.items()),
            'Frozen helper closure changed')
    runtime = {}
    for item in (native['python'], native['python_executable']):
        require(descriptor(item['path']) == item, 'Frozen Python changed')
        runtime[item['path']] = item['sha256']
    for path in (sys.orig_argv[0], sys.executable, report['argv'][0]):
        item = descriptor(path)
        require(report['pins'].get(item['path']) == item['sha256'], 'Python runtime absent from completed native preflight')
        runtime[item['path']] = item['sha256']
    require(report['argv'][1:] == guard['command'][1:]
            and str(Path(guard['command'][0]).resolve()) in runtime, 'Native preflight launch handoff differs')
    require(descriptor(pins['node']['path']) == pins['node'], 'Node independent pin differs')
    helpers = {'driver': descriptor(root / 'benchmarks/f32_painted_interval_wasi.py'),
               'wrapper': descriptor(root / 'benchmarks/run_profile_wasi.mjs'),
               'capture': descriptor(root / 'benchmarks/results/sources/f32-wasi-validation/capture_build.py'),
               'observer': native['guard']}
    require(all(files.get(item['path']) == item['sha256'] for item in helpers.values()), 'WASI helper missing frozen pin')
    inputs = {}
    for name in CASES:
        item = report['inputs'][name]['pdf']
        require(descriptor(item['path']) == {k: item[k] for k in ('path', 'sha256')}
                and len(read(item['path'])) == item['bytes'], 'Native input changed: ' + name)
        inputs[name] = item['sha256']
    require(selection['schema'] == 1 and selection['experiment'] == NAME, 'Wrong selected-build inventory')
    selected = unique(((item['kind'], item['side']), item) for item in selection['builds'])
    require(set(selected) == {(kind, side) for kind in ('native', 'simd', 'scalar') for side in SIDES}
            and len({item['target'] for item in selected.values()}) == 6, 'Missing or shared selected build targets')
    products, all_targets = {}, set()
    for profile, feature in (('simd', '+simd128'), ('scalar', '-simd128')):
        builds = load(pins[profile + '-build-bindings'])
        require(builds['schema'] == 1 and builds['target'] == 'wasm32-wasip1' and builds['profile'] == 'release'
                and builds['target_feature'] == feature and builds['rustflags'] == '-C target-feature=' + feature
                and builds['cargo_encoded_rustflags'] is None, 'Wrong feature build binding')
        options = {'--runtime-profile': profile}
        for side in SIDES:
            item, build = selected[profile, side], builds[side]
            require(build['source_root'] == item['root'] == native['identities'][side]['root']
                    and build['source'] == native['runner_sources'][side]
                    and build['path'] == item['binary'] and build['receipt'] == item['receipt']
                    and build['context']['path'] == item['context'], 'Selection and feature bindings disagree')
            context = load(build['context'])
            final = load({'path': build['receipt'], 'sha256': build['receipt_sha256']})
            complete(final)
            target = context['CARGO_TARGET_DIR']
            require(target == item['target'] == str(Path(target).resolve()) and target not in all_targets,
                    'Wrong or shared feature target')
            all_targets.add(target)
            require(context['capture_sha256'] == helpers['capture']['sha256']
                    and context['RUSTFLAGS'] == builds['rustflags'] and context['CARGO_ENCODED_RUSTFLAGS'] is None
                    and context['CARGO_BUILD_JOBS'] == '1' and context['root'] == item['root']
                    and context['capture_argv'][1:4] == ['-B', helpers['capture']['path'], item['context']]
                    and str(Path(context['capture_argv'][0]).resolve()) in runtime, 'Build capture/feature differs')
            require(final['source_before'] == build['source'] and final['source_root'] == final['cwd'] == item['root']
                    and final['artifacts'].get(item['context']) == build['context']['sha256']
                    and final['artifacts'].get(item['binary']) == build['sha256'] == digest(item['binary'])
                    and item['binary'] == str(Path(target) / 'wasm32-wasip1/release/examples/scene_index_profile.wasm'),
                    'Unbound WASI module/context/source')
            options.update({'--' + side + '-source': item['root'], '--' + side + '-wasm': item['binary'],
                            '--' + side + '-build-receipt': item['receipt']})
        output = str(getattr(args, profile + '_output').resolve())
        require(not Path(output).exists(), 'Run output already exists')
        options.update({'--bindings': pins['bindings']['path'], '--bindings-sha256': pins['bindings']['sha256'],
            '--build-bindings': pins[profile + '-build-bindings']['path'],
            '--build-bindings-sha256': pins[profile + '-build-bindings']['sha256'],
            '--plan': native['plan']['path'], '--plan-sha256': native['plan']['sha256'],
            '--node': pins['node']['path'], '--output': output})
        argv = [native['python']['path'], '-B', helpers['driver']['path'],
                *[value for pair in options.items() for value in pair]]
        require(len(options) == 15, 'Producer option count changed')
        products[profile] = {'schema': 1, 'experiment': NAME + '-wasi-diagnostic', 'runtime_profile': profile,
            'producer_argv': argv, 'python': native['python'], 'node': pins['node'], **helpers,
            'runtime_files_sha256': runtime, 'native_bindings': pins['bindings'], 'frozen_plan': native['plan'],
            'build_bindings': pins[profile + '-build-bindings'], 'inputs_sha256': inputs}
    destination = args.output.resolve()
    run_dirs = [args.simd_output.resolve(), args.scalar_output.resolve()]
    require(len(set(run_dirs + [destination])) == 3
            and all(not a.is_relative_to(b) and not b.is_relative_to(a)
                    for i, a in enumerate(run_dirs + [destination]) for b in (run_dirs + [destination])[i+1:]),
            'Preparation/run directories must be separate, non-nested paths')
    # Outputs are preparation evidence only, never inserted into the frozen imported helper closure.
    destination.mkdir(parents=True, exist_ok=False)
    outputs = {}
    for profile, value in products.items():
        for suffix, data in (('audit-pins.json', value), ('argv.json', value['producer_argv'])):
            path = destination / (profile + '-' + suffix)
            with path.open('x') as handle:
                json.dump(data, handle, indent=2)
                handle.write('\n')
            outputs[path.name] = descriptor(path)
    manifest = {'schema': 1, 'status': 'prepared-not-launched', 'admission': False, 'inputs': pins,
        'plan': native['plan'], 'helper': descriptor(__file__), 'argv': sys.orig_argv,
        'runtime_files_sha256': runtime, 'outputs': outputs,
        'limitations': ['This preparation is not an independent native112 pixel audit or pilot admission.',
            'Freeze both audit-pins externally before launch; final WASI receipt/guard are supplied to the separate auditor afterward.',
            'No source changes, imports of product/helper modules, subprocesses or launches are performed.']}
    with (destination / 'manifest.json').open('x') as handle:
        json.dump(manifest, handle, indent=2)
        handle.write('\n')
    print(json.dumps(descriptor(destination / 'manifest.json')))


if __name__ == '__main__':
    main()
