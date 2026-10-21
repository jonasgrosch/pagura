"""Publish the coordinator's reviewed partial retention; never collect samples."""
import copy
import json
from pathlib import Path
import statistics
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'benchmarks'))
from evaluate_n16_output import load, require, validate_guard
from evaluate_n16_painted_interval_wasi import file_hash, success
from benchkit.statistics import quantile
from paired_render import source_identity

RESULTS = ROOT / 'benchmarks/results'
LOCAL = RESULTS / 'local/n16-painted-interval-recovery'
ORIGINAL = RESULTS / 'local/n16-painted-interval'


def pin(path):
    return {'path': str(path), 'sha256': file_hash(path)}


def main():
    analysis_path = LOCAL / 'confirmation-001-analysis.json'
    analysis = load(analysis_path)
    audit_receipt = load(LOCAL / 'confirmation-001-audit-receipt.json')
    success(audit_receipt)
    validate_guard(load(LOCAL / 'confirmation-001-audit-observer/guard.json'), 'strict')
    for path, digest in audit_receipt['artifacts'].items():
        require(file_hash(Path(path)) == digest, 'audit artifact changed')
    require(audit_receipt['artifacts'][str(analysis_path)] == file_hash(analysis_path), 'unbound analysis')
    report_path = LOCAL / 'confirmation-001/report.json'
    require(file_hash(report_path) == analysis['report_sha256'], 'report changed after audit')
    report = load(report_path)
    quality = load(ORIGINAL / 'quality-002.json')
    staged = load(ORIGINAL / 'staged-check-001/aggregate.json')
    require(analysis['independently_verified_outputs'] == 21320
            and not analysis['background_contention_observed']
            and analysis['target_latency_passes'] == 2, 'reviewed outcome differs')
    require(quality['status'] == 'passed' and all(v == 'passed' for v in quality['gates'].values())
            and quality['source'] == audit_receipt['source_before']
            and staged['status'] == 'verified-staged-check' and staged['stage_count'] == 14
            and staged['source'] == quality['source'],
            'mandatory validation is incomplete')
    identity = report['identities']['candidate']
    require(source_identity(ROOT) == {k: identity[k] for k in ('root', 'commit', 'source_sha256')},
            'publication product changed')
    result = copy.deepcopy(analysis)
    result.update(status='retained-experimental-partial-win', identities=report['identities'],
                  full_experiment_pass=False, release_admission=False,
                  candidate_delta='Skip final N16 page output outside a conservative validated painted interval; eager initialization, composition precision and charges unchanged.',
                  full_check={'all_checks_passed': True, 'stage_count': 14,
                              'execution': 'Separate guarded stages on one source freeze, not one uninterrupted check.py process.',
                              'aggregate': pin(ORIGINAL / 'staged-check-001/aggregate.json')},
                  mandatory_validation={'gates': quality['gates'], 'quality': pin(ORIGINAL / 'quality-002.json'),
                                        'manifest': pin(ORIGINAL / 'quality-002-manifest.json')})
    rows = {(c['case'], c['mode'], c['operation']): c for c in report['cells']}
    for cell in result['cells']:
        raw = rows[(cell['case'], cell['mode'], cell['operation'])]
        cell['sampled_peak_rss_distribution_bytes'] = {}
        for side in ('baseline', 'candidate'):
            values = [p[side]['process']['peak_rss_bytes'] for p in raw['pairs'] if p['pair'] >= 0]
            require(len(values) == 200 and all(type(v) is int and v > 0 for v in values), 'invalid RSS samples')
            cell['sampled_peak_rss_distribution_bytes'][side] = {
                'samples': len(values), 'min': min(values), 'median': statistics.median(values),
                'p95': quantile(values, .95), 'max': max(values)}
    supported = [c for c in result['cells'] if c['role'] == 'protected'
                 and (c['latency']['median_lower'] > 1.02 or c['latency']['p95_lower'] > 1.02)]
    require({(c['case'], c['mode'], c['operation']) for c in supported}
            == {('opaque-tiny', mode, 'cold') for mode in ('exact', 'competitive')}, 'reviewed regression set differs')
    result['retention_decision'] = {
        'decision': 'retain-experimentally',
        'reason': 'Both original full-render targets reproduce large median and p95 benefits, and mandatory independent accuracy, error/resource, native/WASI layout and portability checks pass. Preserve this useful incremental win on the experimental stack.',
        'restriction': 'Experimental branch only. No main, deployed/default, release, general memory-neutrality or competitor promotion. The two supported tiny cold regressions and every protected/RSS miss remain unresolved and must be repaired or evaluated under a separately justified workload restriction before broader promotion.',
        'supported_protected_regressions': supported,
        'protected_latency_misses': 50 - result['protected_latency_passes'],
        'sampled_rss_misses': 52 - result['memory_passes'],
        'rss_interpretation': 'Observed maxima fail the unchanged <=1.00 gate in 24 cells. Unchanged layouts, heap payload and allocation charges do not erase these misses or establish their cause.',
        'further_sampling_authorized_by_this_record': False}
    evidence = [analysis_path, report_path, LOCAL / 'confirmation-001-receipt.json',
        LOCAL / 'confirmation-001-observer/guard.json', LOCAL / 'confirmation-001-audit-receipt.json',
        LOCAL / 'confirmation-001-audit-observer/guard.json', LOCAL / 'confirmation-001-launch.json',
        LOCAL / 'confirmation-config.json', LOCAL / 'freeze-001-receipt.json', LOCAL / 'dryrun-001-receipt.json',
        RESULTS / 'n16-painted-interval-recovery-plan.json', RESULTS / 'n16-painted-interval-recovery-bindings.json',
        ORIGINAL / 'pilot-001-analysis.json', ORIGINAL / 'wasi-001-analysis.json',
        ORIGINAL / 'native-preflight-002/report.json', ORIGINAL / 'confirmation-001-receipt.json',
        ORIGINAL / 'confirmation-001-observer/guard.json', Path(__file__).resolve()]
    result['evidence'] = [pin(path) for path in evidence]
    result['publication_source_files'] = {name: file_hash(ROOT / name) for name in (
        'crates/pagura-render/src/compositor/narrow.rs', 'crates/pagura-render/src/compositor/narrow_tests.rs',
        'crates/pagura-render/src/compositor/output.rs', 'crates/pagura-render/src/tests.rs',
        'crates/pagura-render/src/tests/painted_interval.rs')}
    result['limitations'] = [
        'Synthetic incremental comparison against a9814ee4; not latest main, overall session speedup or a competitor ranking.',
        'All 21320 outputs pass fixed independent references; this is not a proof for every PDF or continuous geometry.',
        'Marginal bootstrap bounds come from the pinned harness and were not independently recomputed; raw pairing, values, point estimates and output artifacts were independently checked.',
        'Known-process observation cannot exclude brief jobs between scans or ordinary desktop interference.',
        'Original interrupted confirmation is wholly excluded; original launch allowance remains exhausted. No historical, pilot or WASI samples are pooled.',
        'Prepublication commit labels identify measured overlays; product fingerprints and file hashes establish correspondence to the later source commit.',
        'The frozen earlier validation ledger is historical; this new record supersedes its pending-performance status without changing its pinned bytes.']
    output = RESULTS / 'n16-painted-interval-experiment.json'
    with output.open('x') as stream:
        stream.write(json.dumps(result, indent=2, allow_nan=False) + '\n')
    lines = ['# N16 painted-interval confirmation', '',
        'Retained as a partial experimental win on 2026-10-21. Full admission fails.', '',
        'Both targets passed; protected latency passed 8/50 cells and sampled peak RSS passed 28/52. '
        'All 21,320 independently checked outputs passed. No observed competing compute.', '',
        '| Target | Baseline median ms | Candidate median ms | Less render time | Median ratio upper | p95 ratio upper | Peak RSS ratio |',
        '| --- | ---: | ---: | ---: | ---: | ---: | ---: |']
    for c in result['cells']:
        if c['role'] == 'target':
            l = c['latency']
            lines.append(f"| {c['mode']} full | {c['baseline_median_ms']:.6f} | {c['candidate_median_ms']:.6f} | {(1-l['median_ratio'])*100:.2f}% | {l['median_upper']:.6f} | {l['p95_upper']:.6f} | {c['peak_rss_ratio']:.6f} |")
    lines += ['', 'The two supported protected signals are the tiny opaque cold cases: Competitive '
        '+3.50% median (81.04 to 83.88 microseconds), Exact +4.46% (85.42 to 89.23 microseconds). '
        'Their median lower bounds exceed the unchanged 1.02 limit. The other protected upper-bound '
        'misses remain failures to establish the bound, without a supported lower-bound regression.', '',
        'Peak RSS misses are not excused by unchanged storage. The largest sampled ratio is 1.3229 '
        'for the Competitive outline-knockout control (24.92 to 32.97 MiB). The Competitive target '
        'also misses RSS (23.92 to 26.22 MiB). Causation is unresolved.', '',
        result['retention_decision']['restriction'], '',
        '[Complete machine-readable result, all 52 cells and RSS distributions](n16-painted-interval-experiment.json). '
        '[Independent validation](n16-painted-interval-quality.md). '
        '[Separately declared recovery](../../docs/plans/n16-painted-interval-recovery.md).', '',
        '## All fixed cells', '',
        '| Case | Mode | Operation | Median ratio | Median lower/upper | p95 lower/upper | RSS ratio | Latency/RSS gates |',
        '| --- | --- | --- | ---: | --- | --- | ---: | --- |']
    for c in result['cells']:
        l = c['latency']
        lines.append(f"| {c['case']} | {c['mode']} | {c['operation']} | {l['median_ratio']:.6f} | {l['median_lower']:.6f}/{l['median_upper']:.6f} | {l['p95_lower']:.6f}/{l['p95_upper']:.6f} | {c['peak_rss_ratio']:.6f} | {'pass' if c['median_pass'] and c['p95_pass'] else 'miss'}/{'pass' if c['memory_pass'] else 'miss'} |")
    lines += ['', 'No overall-stack, latest-main, cold-I/O, global-accuracy or competitor-superiority claim. '
        'The 14 authoritative checks passed as separate stages; WASI was a bounded portability diagnostic. '
        'Original interrupted samples remain excluded. Confidence bounds are marginal and not independently recomputed.', '']
    with (RESULTS / 'n16-painted-interval-experiment.md').open('x') as stream:
        stream.write('\n'.join(lines))
    print(json.dumps({'result': pin(output), 'retention': result['retention_decision']['decision'],
        'target_passes': 2, 'protected_passes': result['protected_latency_passes'],
        'rss_passes': result['memory_passes'], 'admission': False}, indent=2))


if __name__ == '__main__':
    main()
