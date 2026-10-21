#!/usr/bin/env python3
"""Freeze this completed, fixed validation campaign; never execute a validation."""
import argparse
import json
from pathlib import Path
import sys

sys.dont_write_bytecode = True
from assemble import REVIEW, regular, require, sha


def descriptor(path):
    path = Path(path).absolute()
    return {'path': str(path), 'sha256': sha(path)}


def read(path):
    return json.loads(regular(path))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--quality-output', required=True, type=Path)
    parser.add_argument('--source-review', required=True, type=Path)
    args = parser.parse_args()
    root, output = args.root.resolve(strict=True), args.output.absolute()
    require(Path.cwd() == root and not output.exists() and not args.quality_output.exists(), 'new destinations and candidate cwd required')
    require(all(path.is_absolute() for path in (args.root, args.output, args.quality_output, args.source_review)), 'absolute arguments required')
    local, sources = root / 'benchmarks/results/local/n16-painted-interval', root / 'benchmarks/results/sources'
    parent = Path(__file__).resolve().parent
    staged = local / 'staged-check-001'
    stage_plan = read(staged / 'plan.json')
    source = stage_plan['source']
    prep = read(local / 'diagnostics-006/preparation.json')
    identities = {side: value['before'] for side, value in prep['roots'].items()}

    def job(name, report_path, *, log=None):
        receipt_path = local / (name + '-receipt.json')
        guard_path = local / (name + '-observer/guard.json')
        receipt, guard = read(receipt_path), read(guard_path)
        result = {'receipt': descriptor(receipt_path), 'guard': descriptor(guard_path),
                  'report': descriptor(report_path), 'receipt_argv': receipt['argv'], 'guard_command': guard['command'],
                  'source_root': receipt['source_root'], 'runner_source': receipt['source_before'],
                  'snapshots': {event['snapshot']: descriptor(guard_path.parent / event['snapshot']) for event in guard['events']}}
        if 'producer_argv' in read(report_path):
            result['producer_argv'] = read(report_path)['producer_argv']
        if log is not None:
            result.update(log=descriptor(log), receipt_artifacts=['log'], command=read(report_path)['command'])
        return result

    diagnostics = job('diagnostics-006', local / 'diagnostics-006/report.json')
    diagnostic_report = read(local / 'diagnostics-006/report.json')
    diagnostic_source = sources / 'n16-painted-interval-validation/run_v3.py'
    diagnostics.update(preparation=descriptor(local / 'diagnostics-006/preparation.json'),
                       restoration=descriptor(local / 'diagnostic-preparation-004/removed.json'),
                       parser=descriptor(diagnostic_source), build_commands={}, test_commands={})
    for row in diagnostic_report['commands']:
        for suffix, field in (('-build.log', 'build_commands'), ('-tests.log', 'test_commands')):
            if row['log'].endswith(suffix):
                diagnostics[field][row['log'][:-len(suffix)]] = row['command']
    precision = job('precision-host-001', local / 'precision-host-001/report.json')
    precision['builds'] = {}
    for target in ('native', 'wasi'):
        name = f'precision-{target}-build-001'
        path = root / 'benchmarks/results/local/main-reconciliation' / ('n16-' + name)
        precision['builds'][target] = job(name, path.with_suffix('.json'), log=path.with_suffix('.log'))
    reach = job('reach-r2-001', local / 'reach-r2-001/report.json')
    reach_report = read(local / 'reach-r2-001/report.json')
    path = root / 'benchmarks/results/local/main-reconciliation/n16-reach-r2-build-001'
    reach.update(preparation=descriptor(local / 'reach-r2-preparation-001/preparation.json'),
                 restoration=descriptor(local / 'reach-r2-preparation-001/removed.json'),
                 inputs=reach_report['inputs'],
                 build=job('reach-r2-build-001', path.with_suffix('.json'), log=path.with_suffix('.log')))
    failures = []
    for receipt_path in sorted(local.glob('*-receipt.json')):
        receipt = read(receipt_path)
        if receipt['status'] != 'completed':
            name = receipt_path.name[:-len('-receipt.json')]
            evidence = [descriptor(receipt_path)]
            guard_path = local / (name + '-observer/guard.json')
            if guard_path.exists():
                evidence.append(descriptor(guard_path))
            failures.append({'id': name, 'report': descriptor(receipt_path), 'status': receipt['status'],
                             'reason': 'Retained terminal unsuccessful attempt; excluded from all passing gates.', 'evidence': evidence})
    failures.append({'id': 'reach-001-R1', 'report': descriptor(local / 'reach-001/report.json'),
                     'status': read(local / 'reach-001/report.json')['status'],
                     'reason': 'Original fixed six-row Competitive interval hypothesis failed; R2 is a separately frozen structural revision.',
                     'evidence': [descriptor(local / 'reach-001-receipt.json'), descriptor(local / 'reach-001-observer/guard.json')]})
    paths = {Path(__file__).resolve(), parent / 'assemble.py', args.source_review.resolve(),
             Path(sys.executable).resolve(), Path(sys.orig_argv[0]).resolve()}
    paths.update((root / 'tools').rglob('*.py'))
    paths.update((root / 'benchmarks').glob('*.py'))
    for relative in ('n16-staged-check', 'n16-painted-interval-validation', 'n16-painted-interval-reach-r2'):
        paths.update((sources / relative).glob('*.py'))
    review_files = {str(path): sha(path) for path in sorted((root / 'crates/pagura-render/src').rglob('*.rs'))}
    manifest = {'schema': 1, 'experiment': 'n16-painted-interval', 'root': str(root), 'source': source,
        'identities': identities, 'files_sha256': {str(path.resolve()): sha(path) for path in sorted(paths)},
        'aggregator_argv': [str(Path(sys.orig_argv[0]).resolve()), str(parent / 'assemble.py'), '--manifest', str(output),
                            '--manifest-sha256', '{manifest_sha256}', '--output', str(args.quality_output)],
        'helpers': {'staged': descriptor(sources / 'n16-staged-check/staged_check.py'),
                    'diagnostics': descriptor(diagnostic_source), 'reach': descriptor(sources / 'n16-painted-interval-reach-r2/run.py')},
        'staged': {'plan': descriptor(staged / 'plan.json'), 'aggregate': descriptor(staged / 'aggregate.json')},
        'diagnostics': diagnostics, 'precision': precision, 'reach': reach, 'failed_attempts': failures,
        'historical_ledger': descriptor(root / 'benchmarks/results/n16-painted-interval-validation.json'),
        'source_review': {'artifact': descriptor(args.source_review), 'source': source, 'status': 'reviewed',
            'checks': {key: True for key in REVIEW}, 'files_sha256': review_files,
            'notes': 'Explicit coordinator attestation supplied with --source-review; freeze does not derive source semantics from a test count.'},
        'limitations': [
            'Aggregation of separately authenticated checks, not execution or independent rederivation of every test assertion.',
            'All 14 authoritative commands passed as separate stages, not one uninterrupted check.py invocation.',
            'R1 remains failed. R2 is a post-discovery source-derived revision with three fresh controls, not repair of the original hypothesis.',
            'R2 covers 24 native full requests, not all 52 benchmark cells or WASI route instrumentation.',
            'Host protocol evidence covers Exact; both-mode coverage comes from the renderer tests.',
            'Host binary hashes must agree with their original final build receipts and the later host probe.',
            'Source/layout-cfg/writer review is an explicit trusted coordinator attestation, not an exhaustive all-input proof.',
            'Guard snapshot names originate in final receipt-bound guards; snapshot hashes are added by this frozen aggregate input manifest.',
            'Known-process observation and cooperative runner locks do not establish OS-level isolation.',
            'No process RSS, peak-stack, timing, release admission or superiority claim. The 104-output native preflight remains separately required.']}
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open('x') as stream:
        stream.write(json.dumps(manifest, indent=2, allow_nan=False) + '\n')
    print(sha(output))


if __name__ == '__main__':
    main()
