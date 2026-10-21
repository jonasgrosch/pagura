"""Assemble one reviewed F32 quality artifact; no render, build or measurement."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import sys

sys.dont_write_bytecode = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('root', 'request', 'output'):
        parser.add_argument('--' + name, required=True, type=Path)
    parser.add_argument('--request-sha256', required=True)
    args = parser.parse_args()
    root = args.root.resolve(strict=True)
    sys.path.insert(0, str(root / 'benchmarks'))
    from f32_interval_admission import (GATES, NAME, ROOT, decode, descriptor, file_hash,
                                       pin, pinned, read, require, success, validate_guard, verify_core)
    require(root == ROOT, 'Wrong source checkout')
    request = pinned(args.request.resolve(), args.request_sha256)
    require(request['schema'] == 1 and request['experiment'] == NAME
            and request['status'] == 'reviewed-for-quality-assembly'
            and isinstance(request['review_reason'], str) and request['review_reason'].strip(), 'Explicit reviewed request required')
    inputs = pin(request['input_bindings'])
    require(inputs['stage'] == 'preflight-inputs', 'Immutable input-stage bindings required')
    verify_core(inputs)
    require(request['helpers']['assembler']['path'] == str(Path(__file__).resolve())
            and file_hash(Path(__file__).resolve()) == request['helpers']['assembler']['sha256'], 'Wrong assembly helper')
    identities = {side: {k: row[k] for k in ('root', 'commit', 'source_sha256')}
                  for side, row in inputs['identities'].items()}
    source = inputs['runner_sources']['candidate']
    run_id, run_receipt = os.environ.get('PAGURA_RUN_ID'), os.environ.get('PAGURA_RUN_RECEIPT')
    require(run_id and run_receipt, 'Assemble inside the owned coordinator runner')
    live = decode(read(Path(run_receipt)))
    require(live['status'] == 'running' and live['run_id'] == run_id
            and live['source_root'] == str(ROOT) and live['source_before'] == source, 'Live quality runner source differs')
    retained = {}

    def retain(item, parse=False):
        path = Path(item['path'])
        require(path.is_absolute() and path == path.resolve() and file_hash(path) == item['sha256'], 'Evidence pin differs')
        retained[str(path)] = item['sha256']
        return decode(read(path)) if parse else path

    def module(name, item):
        path = retain(item)
        spec = importlib.util.spec_from_file_location(name, path)
        result = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(result)
        return result

    def job(item, expected_source):
        receipt, guard = retain(item['receipt'], True), retain(item['guard'], True)
        success(receipt)
        validate_guard(guard, 'strict')
        child = item['producer_argv']
        require(isinstance(child, list) and child and all(isinstance(x, str) for x in child), 'Missing reviewed child argv')
        argv, cwd = receipt['argv'], Path(receipt['cwd']).resolve()
        require(receipt['source_root'] == str(ROOT) and cwd == ROOT and receipt['source_before'] == expected_source
                and guard['command'] == child and len(argv) >= 6 and argv[0] == inputs['python']['path']
                and (cwd / argv[1]).resolve() == Path(inputs['guard']['path'])
                and argv[2] == '--evidence-dir' and (cwd / argv[3]).resolve() == Path(item['guard']['path']).parent
                and argv[4] == '--' and argv[5:] == child, 'Source or strict launch chain differs')
        for key in ('artifact', 'guard'):
            retain(item[key])
            require(receipt['artifacts'].get(item[key]['path']) == item[key]['sha256'], 'Final receipt misses artifact')
        return receipt

    retain({'path': str(args.request.resolve()), 'sha256': args.request_sha256})
    retain(request['input_bindings'])
    retain(request['helpers']['assembler'])
    # Derive the actual14 command descriptions from the same pinned check helper.
    full = request['evidence']['repository_validation']
    job(full, source)
    check = module('f32_quality_check', request['helpers']['check'])
    require(request['helpers']['check']['path'] == str(ROOT / 'tools/check.py'), 'Wrong check helper')
    expected = ['Checking: ' + check.describe_command(c, e) for c, e in check.planned_commands(jobs=1)]
    log = read(Path(full['artifact']['path'])).decode('utf-8')
    lines = log.splitlines()
    success_line = 'All checks passed. This is not a complete PDF conformance or performance certification.'
    require(len(expected) == 14 and [s for s in lines if s.startswith('Checking: ')] == expected
            and lines.count(success_line) == 1, 'Full check did not complete the exact14 commands')
    after = [line for line in lines[lines.index(success_line) + 1:] if line]
    footer = 'contention guard: completed; evidence: ' + str(Path(full['guard']['path']).parent)
    require(after in ([], [footer]), 'Unexpected output after final check success')

    # Reuse the archived parser on every actual raw six-arm test transcript.
    diagnostic = request['evidence']['six_arm_diagnostics']
    receipt = job(diagnostic, request['diagnostic_runner_source'])
    report = retain(diagnostic['artifact'], True)
    before, installed, removed = (retain(request['diagnostic_preparation'][k], True) for k in ('before', 'installed', 'removed'))
    helper = module('f32_quality_diagnostics', request['helpers']['diagnostics'])
    require(report['status'] == 'verified-tests-and-diagnostics' and report['layout_gate_passed'] is True
            and report['expected_builds'] == report['expected_test_runs'] == 6
            and report['producer_argv'] == diagnostic['producer_argv'] and report['run_id'] == receipt['run_id']
            and report['receipt'] == diagnostic['receipt']['path']
            and report['preparation_sha256'] == request['diagnostic_preparation']['installed']['sha256']
            == removed['preparation_sha256'] and removed['restored'] is True and removed['removed'] == identities,
            'Six-arm completion/preparation/restoration differs')
    for side in ('baseline', 'candidate'):
        row = installed['roots'][side]
        require(before['roots'][side] == {k: v for k, v in row.items() if k != 'installed'}
                and row['before'] == identities[side]
                and report['source_before'][side] == report['source_after'][side] == row['installed'],
                'Before/installed/after product correspondence differs')
        for name, digest in row['files'].items():
            require(file_hash(Path(identities[side]['root']) / name) == digest, 'Owned source file was not restored')
    require(before['patches'] == installed['patches'] and report['fixtures_before'] == report['fixtures_after']
            and report['fixtures_before']['baseline'] and report['fixtures_before']['baseline'] == report['fixtures_before']['candidate'],
            'Patch/fixture correspondence differs')
    for item in installed['patches']:
        retain(item)
    for side, mapping in report['fixtures_after'].items():
        for name, digest in mapping.items():
            require(file_hash(Path(identities[side]['root']) / name) == digest, 'Generated fixture changed')
    directory = Path(diagnostic['artifact']['path']).parent
    for relative, entry in report['artifacts'].items():
        path = (directory / relative).resolve()
        require(path.is_relative_to(directory) and path.stat().st_size == entry['bytes'], 'Invalid diagnostic artifact extent')
        retain({'path': str(path), 'sha256': entry['sha256']})
    for path, digest in report['bound_files_sha256'].items():
        retain({'path': path, 'sha256': digest})
    require(len(report['observations']) == 6 and {(x['arm'], x['platform']) for x in report['observations']}
            == {(a, p) for a in helper.ARMS for p in helper.PLATFORMS}, 'Incomplete six-arm inventory')
    for command in report['commands']:
        process = command['process']
        require(command['status'] == 'returned' and type(process.get('exit_code')) is int
                and process['exit_code'] == 0 and process.get('status') != 'timeout', 'Diagnostic child failed')
    layouts, comparisons = [], []
    for platform in helper.PLATFORMS:
        pair = {}
        for arm in helper.ARMS:
            row = next(x for x in report['observations'] if (x['arm'], x['platform']) == (arm, platform))
            parsed = helper.parse_tests(read(directory / f'{arm}/{platform}-tests.log').decode('utf-8'), arm, platform)
            require(all(row[k] == v for k, v in parsed.items()) and len(row['pi_q4']) == 47
                    and file_hash(Path(row['binary'])) == row['binary_sha256'], 'Raw diagnostics/binary differs')
            pair[arm] = parsed
        require(pair['baseline']['pi_q4'] == pair['candidate']['pi_q4'], 'Same-platform47 Q4 records differ')
        deltas = {}
        for name in helper.TYPES:
            baseline, candidate = (pair[arm]['layouts'][name] for arm in helper.ARMS)
            delta = {k: candidate[k] - baseline[k] for k in ('size', 'align')}
            require(all(type(v) is int and v > 0 for d in (baseline, candidate) for v in d.values())
                    and all(v <= 0 for v in delta.values()), 'Layout grew or is unavailable')
            deltas[name] = delta
            layouts.append({'platform': platform, 'type': name, 'baseline': baseline, 'candidate': candidate, 'delta': delta})
        comparisons.append({'platform': platform, 'pi_q4_equal': True, 'layout_deltas': deltas, 'no_layout_growth': True})
    require(comparisons == report['comparisons'] and len(layouts) == 24, 'Reported layout comparisons differ')

    # The root-reviewed record binds the route build/input/owned restoration
    # correspondence; the raw fixed12 route lines are checked here independently.
    route = request['evidence']['route_reach']
    review = request['route_review']
    route_receipt = job(route, review['installed_runner_source'])
    require(review['reviewed'] is True and review['reason'].strip()
            and review['before_product'] == review['after_product'] == identities['candidate']
            and review['before_runner_source'] == review['after_runner_source'] == source
            and review['owned_patch_restoration_verified'] is True, 'Route correspondence not explicitly reviewed')
    route_log = retain(request['route_log'])
    require(route_receipt['artifacts'].get(str(route_log)) == request['route_log']['sha256'], 'Route raw log unbound')
    for item in review['records']:
        retain(item)
    require(review['records'], 'No pinned route build/input/preparation records')
    text = read(route_log).decode('utf-8')
    require(re.search(r'^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; 243 filtered out;', text, re.M),
            'Focused route test did not pass')
    rows = [s.split('PAGURA_F32_ROUTE_V1 ', 1)[1] for s in text.splitlines() if 'PAGURA_F32_ROUTE_V1 ' in s]
    names = ('sparse64', 'image-only', 'f32-shading-dense', 'f32-pattern-dense', 'opaque64', 'default-knockout64')
    require(len(rows) == 12, 'Route does not contain exactly12 cases')
    for row, (name, mode) in zip(rows, ((n, m) for n in names for m in ('Exact', 'Competitive')), strict=True):
        values = dict(re.findall(r'(\w+)=(None|Some\(\d+\.\.\d+\)|\S+)', row))
        wide = name == 'default-knockout64' or (name == 'f32-pattern-dense' and mode == 'Exact')
        glyph = name in ('sparse64', 'opaque64')
        side = 128 if name == 'image-only' else 256
        calls = 0 if wide else (7 if mode == 'Competitive' else 6) if glyph else side
        interval = 'None' if wide else f'Some(59904..{61696 if mode == "Competitive" else 61440})' if glyph else f'Some(0..{side * side})'
        expected = {'case': name, 'mode': mode, 'page': str(2048 if glyph or name == 'default-knockout64' else 512),
                    'side': str(side), 'format': 'F64' if wide else 'N16' if name == 'opaque64' else 'F32',
                    'interval': interval, 'narrow_output_calls': str(calls), 'narrow_output_pixels': str(calls * side)}
        require(all(values.get(k) == v for k, v in expected.items()) and int(values['input_bytes']) > 0, 'Fixed route prediction differs')
        paths, masks, dynamic = (int(values[k]) for k in ('paths', 'masks', 'dynamic'))
        require((paths == masks == dynamic == 0) if wide else
                ((dynamic == 0 and (masks > 0 and paths == 0 if mode == 'Competitive' else paths > 0 and masks == 0))
                 if glyph else paths == masks == 0 and dynamic == side * side), 'Actual writer route differs')
    restorations = [request['diagnostic_preparation']['removed'], *request['route_restorations']]
    require(request['route_restorations'], 'Missing owned route restoration records')
    for item in restorations:
        retain(item)
    retain(request['source_review'])
    verify_core(inputs)
    require(all(file_hash(Path(p)) == h for p, h in retained.items()), 'Quality input changed during assembly')
    result = {'schema': 1, 'experiment': NAME, 'status': 'passed', 'admission': False, 'source': source,
              'producer_argv': sys.orig_argv, 'run_id': run_id, 'run_receipt': run_receipt,
              'gates': {name: 'passed' for name in GATES}, 'layouts': layouts, 'evidence': request['evidence'],
              'restorations': restorations, 'request': descriptor(args.request.resolve()), 'source_review': request['source_review'],
              'input_bindings': request['input_bindings'], 'verified_artifacts_sha256': retained,
              'limitations': ['Raw full-check and six-arm diagnostics are revalidated; route build/input/restoration linkage remains an explicitly reviewed request boundary.',
                             'Route evidence is12 actual first chunks, not full-page reach or a performance/RSS proof.',
                             'Native112 and SIMD336/scalar24 are separately mandatory in verify_admission; no timing or admission is granted here.']}
    with args.output.resolve().open('x') as output:
        json.dump(result, output, indent=2)
        output.write('\n')
    print(args.output.resolve())


if __name__ == '__main__':
    main()
