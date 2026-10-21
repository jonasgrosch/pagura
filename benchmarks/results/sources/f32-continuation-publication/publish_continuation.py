"""Publish the explicitly amended two-cohort F32 result; never collect samples."""
import argparse
import copy
import json
import math
import os
from pathlib import Path
import statistics
import sys

sys.dont_write_bytecode = True
ROOT = Path('/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura')
sys.path.insert(0, str(ROOT / 'benchmarks'))
from f32_interval_admission import GATES, RESULTS, descriptor, evidence, pin, require
from f32_painted_interval_continuation import LOCAL, NAME, validate
from evaluate_n16_output import THRESHOLDS, cell_map, load
from benchkit.statistics import quantile
from paired_render import source_identity


def key(cell):
    return cell['case'], cell['mode'], cell['operation']


def label(cell):
    return {k: cell[k] for k in ('case', 'mode', 'operation', 'role')}


def text(value):
    return str(value).replace('|', '\\|').replace('\r', ' ').replace('\n', ' ')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--decision', required=True, type=Path)
    parser.add_argument('--decision-sha256', required=True)
    args = parser.parse_args()
    decision_pin = {'path': str(args.decision.resolve()), 'sha256': args.decision_sha256}
    decision = pin(decision_pin)
    require(decision['schema'] == 1 and decision['experiment'] == NAME and decision['run'] == 'confirmation-001'
            and decision['decision'] in ('retain-experimentally', 'reject')
            and all(isinstance(decision[k], str) and decision[k].strip() for k in ('reason', 'restriction')),
            'Explicit post-audit coordinator decision required')
    config_pin = decision['confirmation_config']
    config, bindings, amendment = validate(Path(config_pin['path']), config_pin['sha256'])
    audit = decision['audit']
    expected_paths = {'artifact': LOCAL / 'confirmation-001-analysis.json',
                      'receipt': LOCAL / 'confirmation-001-audit-receipt.json',
                      'guard': LOCAL / 'confirmation-001-audit-observer/guard.json'}
    require(all(audit[k]['path'] == str(v) for k, v in expected_paths.items()), 'Wrong final audit evidence')
    command = [bindings['python']['path'], str(ROOT / 'benchmarks/evaluate_f32_painted_interval_continuation.py'),
               'confirmation-001', '--config', config_pin['path'], '--config-sha256', config_pin['sha256']]
    _, analysis, _ = evidence(audit, bindings, command)
    report_pin = {'path': str(LOCAL / 'confirmation-001/report.json'), 'sha256': analysis['report_sha256']}
    run = {'artifact': report_pin,
           'receipt': {'path': str(LOCAL / 'confirmation-001-receipt.json'), 'sha256': analysis['receipt_sha256']},
           'guard': {'path': str(LOCAL / 'confirmation-001-observer/guard.json'), 'sha256': analysis['guard_sha256']}}
    measurement_receipt, report, _ = evidence(run, bindings)
    require(analysis['schema'] == 1 and analysis['experiment'] == NAME
            and analysis['run'] == 'confirmation-001' and analysis['phase'] == 'confirmation'
            and analysis['status'] == 'verified-amended-continuation'
            and analysis['admission'] is False and analysis['independently_verified_outputs'] == 22960
            and analysis['original_process_presence_observed'] is True
            and analysis['continuation_process_presence_observed'] is False
            and analysis['uninterrupted_confirmation'] is False
            and analysis['bindings_sha256'] == config['bindings']['sha256']
            and analysis['plan_sha256'] == bindings['plan']['sha256']
            and analysis['bootstrap_bounds_independently_recomputed'] is False
            and report['status'] == 'complete-amended-exploratory' and report['phase'] == 'confirmation'
            and report['identities'] == bindings['identities'] and report['pairs'] == 200 and report['warmups'] == 5,
            'Final audit/report is incomplete or belongs to another freeze')
    plan, quality = pin(bindings['plan']), pin(bindings['quality']['artifact'])
    require(quality['status'] == 'passed' and quality['gates'] == {name: 'passed' for name in GATES}, 'Mandatory quality incomplete')
    require(cell_map(analysis['cells'], 56) == cell_map(report['cells'], 56) == cell_map(plan['cells'], 56), 'Fixed cell inventory differs')
    identity = {k: bindings['identities']['candidate'][k] for k in ('root', 'commit', 'source_sha256')}
    require(source_identity(ROOT) == identity, 'Publication product changed')
    run_id, receipt_path = os.environ.get('PAGURA_RUN_ID'), os.environ.get('PAGURA_RUN_RECEIPT')
    require(run_id and receipt_path, 'Publish under the owned coordinator runner')
    live = load(Path(receipt_path))
    require(live['status'] == 'running' and live['run_id'] == run_id and live['source_root'] == str(ROOT)
            and live['source_before'] == bindings['runner_sources']['candidate'], 'Publication runner differs')
    result = copy.deepcopy(analysis)
    raw_cells = {key(c): c for c in report['cells']}
    require(report['amendment'] == config['amendment']
            and report['original_report'] == amendment['original']['report']
            and analysis['original_completed_pairs'] == report['original_completed_pairs']
            and analysis['new_completed_pairs'] == report['new_completed_pairs']
            and analysis['abandoned_attempts_count'] == len(report['abandoned_attempts'])
            and len(report['pair_sources']) == 11480
            and [p['sequence'] for p in report['pair_sources']] == list(range(11480))
            and report['original_completed_pairs'] + report['new_completed_pairs'] == 11480,
            'Two-cohort provenance inventory differs')
    supported, latency_misses, rss_misses = [], [], []
    for cell in result['cells']:
        raw, latency = raw_cells[key(cell)], cell['latency']
        require([p['pair'] for p in raw['pairs']] == list(range(-5, 200)), 'Warmup/measured pair inventory changed')
        require(all(type(v) in (int, float) and math.isfinite(v) and v > 0 for v in latency.values()), 'Unavailable latency statistic')
        for statistic in ('median', 'p95'):
            require(latency[statistic + '_lower'] <= latency[statistic + '_upper']
                    and cell[statistic + '_pass'] is (latency[statistic + '_upper'] <= THRESHOLDS[cell['role'] + '_' + statistic + '_upper']),
                    'Reported latency gate differs')
        distributions = {}
        for side in ('baseline', 'candidate'):
            values = [p[side]['process']['peak_rss_bytes'] for p in raw['pairs'] if p['pair'] >= 0]
            require(len(values) == 200 and all(type(v) is int and v > 0 for v in values), 'Unavailable RSS samples')
            distributions[side] = {'samples': 200, 'min': min(values), 'median': statistics.median(values),
                                   'p95': quantile(values, .95), 'max': max(values)}
        ratio = distributions['candidate']['max'] / distributions['baseline']['max']
        require(cell['peak_rss_ratio'] == ratio and cell['memory_pass'] is (ratio <= 1.0), 'Reported RSS gate differs')
        cell['sampled_peak_rss_distribution_bytes'] = distributions
        cell_index = next(i for i, value in enumerate(report['cells']) if key(value) == key(cell))
        origins = [p for p in report['pair_sources'] if p['cell_index'] == cell_index]
        require([p['pair'] for p in origins] == list(range(-5, 200)), 'Per-cell cohort provenance differs')
        cell['cohort_pair_counts'] = {
            segment: {'warmup': sum(p['segment'] == segment and p['pair'] < 0 for p in origins),
                      'measured': sum(p['segment'] == segment and p['pair'] >= 0 for p in origins)}
            for segment in ('original', 'continuation')}
        signals = [s for s in ('median', 'p95') if latency[s + '_lower'] > 1.02]
        if signals:
            supported.append({**label(cell), 'metrics': signals, 'latency': latency})
        if not (cell['median_pass'] and cell['p95_pass']):
            latency_misses.append(label(cell))
        if not cell['memory_pass']:
            rss_misses.append({**label(cell), 'peak_rss_ratio': ratio})
    targets = [c for c in result['cells'] if c['role'] == 'target']
    require({key(c) for c in targets} == {('sparse64', m, 'full') for m in ('exact', 'competitive')}, 'Original target identity differs')
    counts = {'target_latency_passes': sum(c['median_pass'] and c['p95_pass'] for c in targets),
              'protected_latency_passes': sum(c['median_pass'] and c['p95_pass'] for c in result['cells'] if c['role'] == 'protected'),
              'memory_passes': sum(c['memory_pass'] for c in result['cells'])}
    numeric_pass = all(c['median_pass'] and c['p95_pass'] and c['memory_pass'] for c in result['cells'])
    require(all(analysis[k] == v for k, v in counts.items()) and analysis['numeric_gates_pass'] is numeric_pass, 'Audit gate totals differ')
    require(decision['decision'] != 'retain-experimentally' or any(c['latency']['median_upper'] <= .95 for c in targets),
            'Experimental retention lacks the original target median benefit')
    evidence_pins = [decision_pin, *audit.values(), *run.values(), config_pin, config['bindings'], bindings['plan'],
                     config['amendment'], amendment['original_config'], *amendment['original'].values(),
                     descriptor(LOCAL / 'confirmation-001-launch.json'), descriptor(LOCAL / 'confirmation-001-driver-start.json'),
                     descriptor(Path(__file__).resolve())]
    for item in (config_pin, evidence_pins[-3], evidence_pins[-2]):
        require(measurement_receipt['artifacts'].get(item['path']) == item['sha256'], 'Measured launch evidence changed')
    result.update(status='retained-experimental' if decision['decision'] == 'retain-experimentally' else 'rejected',
                  identities=report['identities'], full_experiment_pass=False,
                  original_uninterrupted_protocol_pass=False, amended_numeric_gates_pass=numeric_pass,
                  release_admission=False,
                  retention_decision={**decision, 'promotion_scope': 'experimental-only; no main/default/release promotion',
                                      'further_sampling_authorized_by_this_record': False},
                  latency_gate_misses=latency_misses, sampled_rss_gate_misses=rss_misses,
                  supported_lower_bound_above_1_02=supported,
                  mandatory_validation={'gates': quality['gates'], 'quality': bindings['quality'],
                                        'native_preflight': bindings['native_preflight'], 'wasi': bindings['wasi'],
                                        'wasi_review': bindings['wasi_review']},
                  evidence=evidence_pins,
                  publication={'run_id': run_id, 'run_receipt': receipt_path, 'producer_argv': sys.orig_argv,
                               'source': bindings['runner_sources']['candidate']},
                  candidate_delta='Skip final F32 page output outside a conservative validated painted interval; dense dynamic writers mark their full operation.',
                  method_amendment={'record': config['amendment'], 'original_config': amendment['original_config'],
                                    'selection_basis': 'Every structurally complete original pair retained; unfinished fixed schedule continued, without timing-based selection.',
                                    'original_observer_status': 'invalid-external-contention',
                                    'original_stop_observation': 'Process presence; CPU consumption and timing effect were not established.',
                                    'uninterrupted_confirmation': False},
                  raw_segments=report['segments'], pair_sources=report['pair_sources'],
                  abandoned_attempts=report['abandoned_attempts'],
                  original_completed_pairs=report['original_completed_pairs'],
                  new_completed_pairs=report['new_completed_pairs'],
                  limitations=[
                      'Synthetic incremental comparison against f9738174; no overall-stack, latest-main, competitor or global superiority claim.',
                      'All 22960 outputs were independently checked against fixed fixture references; this is not correctness for every PDF or continuous geometry.',
                      'Marginal bootstrap bounds are retained from the pinned harness, not independently recomputed by the audit or publication helper.',
                      'All 200 measured pairs per cell contribute to RSS distributions; maxima have no confidence interval, and misses have no presumed cause.',
                      'This is a post-stop user-authorized two-cohort method amendment, not uninterrupted or wholly fresh confirmation.',
                      'The original guard stopped on another process being present; neither zero CPU consumption nor actual timing contamination is established.',
                      'Every structurally complete original pair is retained. The terminal incomplete attempt remains recorded and its pair is rerun in full.',
                      'The cohort/time boundary is explicit; unchanged pooled bootstrap thresholds do not establish equivalence to uninterrupted sampling.',
                      'The continuation guard and independent audit are required to pass; observation still cannot exclude brief jobs between scans or ordinary desktop interference.',
                      'WASI is a bounded portability/diagnostic comparison with no calibrated speed or RSS admission.',
                      'No pilot or historical samples are pooled; no further measurement is authorized by this publication.'])
    output = RESULTS / (NAME + '-experiment.json')
    markdown = RESULTS / (NAME + '-experiment.md')
    require(not output.exists() and not markdown.exists(), 'Refuse replacing a published result')
    lines = ['# F32 painted-interval amended confirmation', '', '**Coordinator decision:** ' + text(decision['decision']) + '.', '',
             text(decision['reason']), '', text(decision['restriction']), '',
             'Retention, if selected, is experimental only; no main/default/release promotion.', '',
             'The original segment stopped when its observer detected another process. Process presence alone establishes neither CPU use nor a timing effect. The user explicitly authorized retaining every completed pair and finishing the original fixed schedule; no timing-based sample selection was used. The failed original receipt/guard and any incomplete terminal attempt remain linked in the JSON.', '',
             f"Two cohorts: {report['original_completed_pairs']} complete original pairs and {report['new_completed_pairs']} continuation pairs, including the original five warmups per cell. This is neither uninterrupted nor wholly fresh confirmation. The continuation receipt/guard and combined independent audit passed.", '',
             f"Unchanged numeric gates: target latency {counts['target_latency_passes']}/2; protected latency {counts['protected_latency_passes']}/54; sampled peak RSS {counts['memory_passes']}/56. All 22,960 scheduled outputs independently checked. Numeric all-cell pass: {str(numeric_pass).lower()}. These pooled gates do not remove the cohort limitation. No release admission.", '',
             f"Retain every miss: {len(latency_misses)} latency cells and {len(rss_misses)} RSS cells. {len(supported)} cells have a median or p95 lower bound above 1.02; these are listed in the JSON. Upper-bound misses alone do not establish a regression. RSS misses remain unresolved observations and are not excused by layout evidence.", '',
             '[All statistics, decisions and exact evidence pins](' + output.name + '). Ratios are candidate/baseline; lower is faster or smaller. Bounds are marginal 95% one-sided endpoints.']
    for c in supported:
        signals = ', '.join(f"{s} lower {c['latency'][s + '_lower']:.6f}" for s in c['metrics'])
        lines += ['', f"Supported >2% signal: {c['case']} / {c['mode']} / {c['operation']} ({c['role']}): {signals}."]
    lines += ['',
             '| Case | Mode | Operation | Role | Baseline/candidate median ms | Median ratio [lower, upper] | p95 ratio [lower, upper] | Latency gate |',
             '| --- | --- | --- | --- | ---: | ---: | ---: | --- |']
    for c in result['cells']:
        l = c['latency']
        lines.append(f"| {c['case']} | {c['mode']} | {c['operation']} | {c['role']} | {c['baseline_median_ms']:.6f}/{c['candidate_median_ms']:.6f} | {l['median_ratio']:.6f} [{l['median_lower']:.6f}, {l['median_upper']:.6f}] | {l['p95_ratio']:.6f} [{l['p95_lower']:.6f}, {l['p95_upper']:.6f}] | {'pass' if c['median_pass'] and c['p95_pass'] else 'miss'} |")
    lines += ['', 'RSS values below are min / median / p95 / max MiB from all 200 measured samples in each arm. The unchanged RSS gate uses the ratio of maxima.', '',
              '| Case | Mode | Operation | Baseline RSS MiB | Candidate RSS MiB | Max ratio | RSS gate |',
              '| --- | --- | --- | ---: | ---: | ---: | --- |']
    for c in result['cells']:
        d = c['sampled_peak_rss_distribution_bytes']
        values = [' / '.join(f'{d[s][k] / 2**20:.3f}' for k in ('min', 'median', 'p95', 'max')) for s in ('baseline', 'candidate')]
        lines.append(f"| {c['case']} | {c['mode']} | {c['operation']} | {values[0]} | {values[1]} | {c['peak_rss_ratio']:.6f} | {'pass' if c['memory_pass'] else 'miss'} |")
    lines += ['', *['- ' + s for s in result['limitations']], '']
    require(source_identity(ROOT) == identity and all(descriptor(item['path']) == item for item in evidence_pins), 'Publication inputs/source changed')
    with output.open('x') as stream:
        stream.write(json.dumps(result, indent=2, allow_nan=False) + '\n')
    with markdown.open('x') as stream:
        stream.write('\n'.join(lines))
    print(json.dumps({'json': descriptor(output), 'markdown': descriptor(markdown), 'decision': decision['decision'],
                      **counts, 'release_admission': False}, indent=2))


if __name__ == '__main__':
    main()
