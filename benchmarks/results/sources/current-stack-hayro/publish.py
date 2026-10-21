"""Publish all audited cells and the predeclared paired primary-time analysis."""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import statistics
import sys

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[4]
PACKAGE = Path(__file__).resolve().parent
LOCAL = ROOT / 'benchmarks/results/local/current-stack-hayro'
sys.path[:0] = [str(ROOT / 'benchmarks'), str(ROOT / 'tools')]
from benchkit.statistics import ratio_bounds
from paired_render import source_identity


def pin(path):
    path = Path(path).resolve()
    return {'path': str(path), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}


def load(item):
    assert pin(item['path']) == item
    return json.loads(Path(item['path']).read_bytes())


def completed(receipt):
    assert receipt['status'] == 'completed' and receipt['exit_code'] == receipt['wrapper_exit_code'] == 0
    assert receipt['source_unchanged'] and not receipt['errors'] and not receipt['artifact_errors']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--config-sha256', required=True)
    args = parser.parse_args()
    config_pin = {'path': str(args.config.resolve()), 'sha256': args.config_sha256}
    config = load(config_pin)
    assert source_identity(ROOT) == config['product']
    audit_pin = pin(LOCAL / 'run-001-analysis.json')
    audit = load(audit_pin)
    audit_receipt_pin = pin(LOCAL / 'audit-001-receipt.json')
    audit_receipt = load(audit_receipt_pin)
    completed(audit_receipt)
    assert audit_receipt['artifacts'][audit_pin['path']] == audit_pin['sha256']
    assert audit_receipt['artifacts'][audit['audit_source']['path']] == audit['audit_source']['sha256']
    assert pin(audit['audit_source']['path']) == audit['audit_source']
    audit_guard_pin = pin(LOCAL / 'audit-001-observer/guard.json')
    audit_guard = load(audit_guard_pin)
    assert audit_guard['status'] == 'completed' and not any(e['matches'] for e in audit_guard['events'])
    assert audit_receipt['artifacts'][audit_guard_pin['path']] == audit_guard_pin['sha256']
    assert audit['status'] == 'verified-complete-panel' and audit['config'] == config_pin
    assert (audit['attempted_outputs'], audit['paired_positions'], audit['cell_count']) == (16380, 8190, 78)
    receipt, raw = load(audit['receipt']), load(audit['report'])
    completed(receipt)
    plan_pin = pin(PACKAGE / 'analysis-plan.json')
    plan = load(plan_pin)
    assert receipt['artifacts'][plan_pin['path']] == plan_pin['sha256']
    assert plan['status'] == 'frozen-before-measurement' and plan['primary_metric'] == 'engine_ns'
    assert (plan['resamples'], plan['one_sided_confidence'], plan['seed']) == (5000, .95, 42017)
    assert pin(plan['source']['path']) == plan['source']
    raw_cells = {(c['case'], c['mode']): c for c in raw['cells']}
    cells = copy.deepcopy(audit['cells'])
    assert len(raw_cells) == len(cells) == 78
    for cell in cells:
        pairs = [p for p in raw_cells[(cell['case'], cell['mode'])]['pairs'] if p['pair'] >= 0]
        assert [p['pair'] for p in pairs] == list(range(100))
        values = {s: [p[s].get('stages', {}).get('engine_ns') for p in pairs] for s in ('pagura', 'hayro')}
        cell['primary_bounds'] = {'status': 'unavailable', 'accuracy_qualified': cell['qualification'] == 'both-engines-correct'}
        if all(type(v) is int and v > 0 for data in values.values() for v in data):
            bounds = ratio_bounds(values['pagura'], values['hayro'], resamples=5000, confidence=.95, seed=42017)
            point = cell['metrics']['engine_ns']['ratios']
            assert bounds['median_ratio'] == point['median'] and bounds['p95_ratio'] == point['p95']
            cell['primary_bounds'].update(status='calculated-from-all-100-pairs', **bounds,
                median_target_pass=bounds['median_upper'] <= .8,
                p95_target_pass=bounds['p95_upper'] <= .8,
                bootstrap_bounds_independently_recomputed=False)
    summaries = []
    for cohort, total in (('legacy19', 19), ('interval20', 20)):
        for mode in ('exact', 'competitive'):
            selected = [c for c in cells if (c['cohort'], c['mode']) == (cohort, mode)]
            assert len(selected) == total
            qualified = [c for c in selected if c['qualification'] == 'both-engines-correct']
            timed = [c for c in qualified if c['primary_bounds']['status'] == 'calculated-from-all-100-pairs']
            rss = [c['metrics']['peak_rss_bytes']['ratios']['maximum'] for c in qualified
                   if 'ratios' in c['metrics']['peak_rss_bytes']]
            ratios = [c['primary_bounds']['median_ratio'] for c in timed]
            summaries.append({'cohort': cohort, 'mode': mode, 'total_cases': total,
                'pagura_correct_cases': sum(c['accuracy_counts']['pagura'].get('correct', 0) == 105 for c in selected),
                'hayro_correct_cases': sum(c['accuracy_counts']['hayro'].get('correct', 0) == 105 for c in selected),
                'qualified_cases': len(qualified), 'complete_primary_timing_cases': len(timed),
                'median_of_case_median_time_ratios': statistics.median(ratios) if ratios else None,
                'range_of_case_median_time_ratios': [min(ratios), max(ratios)] if ratios else None,
                'faster_median_cases': sum(value < 1 for value in ratios),
                'median_and_p95_20percent_bound_passes': sum(c['primary_bounds']['median_target_pass'] and c['primary_bounds']['p95_target_pass'] for c in timed),
                'sampled_peak_rss_no_larger_cases': sum(value <= 1 for value in rss),
                'rss_cases': len(rss), 'maximum_sampled_peak_rss_ratio': max(rss) if rss else None})
    historical = json.loads((ROOT / 'benchmarks/results/analytic19-conversion.json').read_bytes())
    old = {c['case']: c for c in historical['cells'] if c['competitor'] == 'hayro' and c['accuracy_qualified']}
    comparable = []
    for c in cells:
        if c['mode'] == 'exact' and c['cohort'] == 'legacy19' and c['qualification'] == 'both-engines-correct' and c['case'] in old:
            comparable.append({'case': c['case'], 'old_ratio': old[c['case']]['metrics']['engine_ns']['median_ratio'],
                               'current_ratio': c['primary_bounds'].get('median_ratio')})
    result = {'schema': 1, 'experiment': 'current-stack-hayro', 'status': 'audited-comparison',
        'product': config['product'], 'engines': config['engines'], 'protocol': config['protocol'],
        'summaries': summaries, 'historical_common_exact_cases': comparable, 'cells': cells,
        'accuracy_counts': audit['accuracy_counts'], 'attempted_outputs': 16380,
        'analysis_plan': plan_pin, 'bootstrap_method': plan,
        'bindings': {'config': config_pin, 'audit': audit_pin, 'audit_receipt': audit_receipt_pin,
                     'audit_guard': audit_guard_pin, 'run_receipt': audit['receipt'], 'report': audit['report'],
                     'guard': audit['guard'], 'journal': audit['journal'], 'publisher': pin(__file__)},
        'release_admission': False, 'limitations': audit['limitations'] + [
            'Primary bootstrap uses the prospectively frozen existing statistics implementation. Bounds are not independently recalculated by the raw-evidence auditor.',
            'Bounds are marginal 95% one-sided endpoints, without a family-wide or temporal-dependence guarantee.',
            'Historical ratios come from an earlier run and adapter build; changes in ratios do not isolate a causal renderer speedup.',
            'Case-ratio medians are unweighted descriptions of each matching subset; cohort and mode subsets may differ.']}
    target = ROOT / 'benchmarks/results/current-stack-hayro.json'
    markdown = target.with_suffix('.md')
    lines = ['# Current combined stack versus Hayro', '',
        'Product: `402e3afe`. Hayro: standalone 0.8.0. All 78 cells and 16,380 attempts were independently audited.',
        '100 measured pairs and five warmups per case/mode; fresh process, page zero, 72 DPI, independent RGB over white.',
        'Primary time includes opening and rendering/preparation. Ratios are Pagura/Hayro; lower is faster or smaller.',
        'The renderer was unchanged. This selected desktop comparison does not grant release or global superiority admission.', '',
        '| Panel | Mode | Pagura correct | Hayro correct | Both correct | Median case time ratio | Faster median cases | Peak RSS no larger |',
        '| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |']
    for summary in summaries:
        ratio = summary['median_of_case_median_time_ratios']
        ratio_text = f'{ratio:.4f}' if ratio is not None else 'unavailable'
        lines.append(f"| {summary['cohort']} | {summary['mode']} | {summary['pagura_correct_cases']}/{summary['total_cases']} | {summary['hayro_correct_cases']}/{summary['total_cases']} | {summary['qualified_cases']} | {ratio_text} | {summary['faster_median_cases']}/{summary['complete_primary_timing_cases']} | {summary['sampled_peak_rss_no_larger_cases']}/{summary['rss_cases']} |")
    lines += ['', 'Accuracy requires every warmup and measured output to match the frozen independent RGB policy. A mismatch may reflect a policy difference; it is not alone proof of a PDF-standard violation.',
        'Every case follows, including incorrect or failed outputs. Quality-unqualified times are descriptive and cannot support a correctness-qualified speed claim.', '',
        '| Case | Mode | Accuracy | Pagura/Hayro median ms | Median time ratio | p95 time ratio | Median upper | p95 upper | Max RSS ratio |',
        '| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |']
    def number(value):
        return f'{value:.4f}' if value is not None else 'unavailable'
    for c in sorted(cells, key=lambda c: (c['cohort'], c['case'], c['mode'])):
        metric, bounds = c['metrics']['engine_ns'], c['primary_bounds']
        times = '/'.join(number(metric.get('sides', {}).get(s, {}).get('median', 0) / 1e6)
                         if s in metric.get('sides', {}) else 'unavailable' for s in ('pagura', 'hayro'))
        ratios = metric.get('ratios', {})
        rss = c['metrics']['peak_rss_bytes'].get('ratios', {}).get('maximum')
        lines.append(f"| {c['case']} | {c['mode']} | {c['qualification']} | {times} | {number(ratios.get('median'))} | {number(ratios.get('p95'))} | {number(bounds.get('median_upper'))} | {number(bounds.get('p95_upper'))} | {number(rss)} |")
    lines += ['', '[All distributions, mismatch details and evidence bindings](current-stack-hayro.json).', '',
              *['- ' + v for v in result['limitations']], '']
    assert source_identity(ROOT) == config['product']
    for item in result['bindings'].values():
        assert pin(item['path']) == item
    with target.open('x') as stream:
        stream.write(json.dumps(result, indent=2, allow_nan=False) + '\n')
    with markdown.open('x') as stream:
        stream.write('\n'.join(lines))
    print(json.dumps({'summaries': summaries, 'result': pin(target), 'markdown': pin(markdown)}, indent=2))


if __name__ == '__main__':
    main()
