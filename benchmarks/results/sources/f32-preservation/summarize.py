"""Summarize the audited fixed trial without filtering cells or inferring admission."""
import hashlib
import json
from pathlib import Path
import statistics

ROOT = Path.cwd()
LOCAL = ROOT / 'benchmarks/results/local/f32-preservation'
sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
load = lambda path: json.loads(Path(path).read_text())

audit = load(LOCAL / 'audit-001.log')
assert audit['status'] == 'audited-completed-diagnostic'
assert (audit['preflight_outputs'], audit['cost_outputs']) == (76, 2736)
plan = load(ROOT / 'benchmarks/results/f32-preservation-cost-plan.json')
bindings = load(ROOT / 'benchmarks/results/f32-preservation-cost-bindings.json')
evidence = {}
for label in ('renderer-002', 'full-check-001', 'build-001', 'preflight-001',
              'cost-001', 'wasi-build-001', 'wasi-tests-001', 'wasi-budget-001', 'audit-001'):
    receipt_path = LOCAL / (label + '-receipt.json')
    receipt = load(receipt_path)
    guard_path = LOCAL / (label + '-observer/guard.json')
    guard = load(guard_path)
    assert receipt['status'] == 'completed' and receipt['exit_code'] == 0
    assert receipt['source_unchanged'] and not receipt['errors'] and not receipt['artifact_errors']
    assert guard['status'] == 'completed' and guard['exit_code'] == guard['child_exit_code'] == 0
    assert not guard['observation_only'] and all(not event['matches'] for event in guard['events'])
    assert receipt['argv'][receipt['argv'].index('--') + 1:] == guard['command']
    for path, digest in receipt['artifacts'].items():
        assert sha(path) == digest
    evidence[label] = {'receipt': {'path': str(receipt_path), 'sha256': sha(receipt_path)},
                       'guard': {'path': str(guard_path), 'sha256': sha(guard_path)},
                       'scans': guard['scans'], 'artifacts': receipt['artifacts']}

cells = []
for row in audit['summary']:
    b, c = (row['medians'][side] for side in ('baseline', 'candidate'))
    maxima = row['measured_maximum_peak_rss_bytes']
    cells.append({**row, 'candidate_over_baseline': {
        'total_ns': c['total_ns'] / b['total_ns'],
        'render_ns': c['render_ns'] / b['render_ns'],
        'median_peak_rss_bytes': c['peak_rss_bytes'] / b['peak_rss_bytes'],
        'maximum_peak_rss_bytes': maxima['candidate'] / maxima['baseline']}})

mode_summary = {}
for mode in ('exact', 'competitive'):
    selected = [row for row in cells if row['cell'][1] == mode]
    ratios = [row['candidate_over_baseline'] for row in selected]
    mode_summary[mode] = {
        'median_of_cell_total_time_ratios': statistics.median(row['total_ns'] for row in ratios),
        'total_time_ratio_range': [min(row['total_ns'] for row in ratios), max(row['total_ns'] for row in ratios)],
        'median_rss_ratio_range': [min(row['median_peak_rss_bytes'] for row in ratios), max(row['median_peak_rss_bytes'] for row in ratios)],
        'maximum_rss_ratio_range': [min(row['maximum_peak_rss_bytes'] for row in ratios), max(row['maximum_peak_rss_bytes'] for row in ratios)]}

result = {'schema': 1, 'status': 'retained-accuracy-improvement', 'admission': False,
          'experiment': 'f32-preservation-cost', 'identities': bindings['identities'],
          'plan_sha256': sha(ROOT / 'benchmarks/results/f32-preservation-cost-plan.json'),
          'bindings_sha256': sha(ROOT / 'benchmarks/results/f32-preservation-cost-bindings.json'),
          'auditor_sha256': audit['auditor_sha256'], 'audit_pins_sha256': audit['pins_sha256'],
          'quality': {'candidate_correct_outputs': 1406, 'candidate_total_outputs': 1406,
                      'baseline_known_wrong_outputs': 37, 'baseline_total_outputs': 1406,
                      'case': 'paint/text-knockout-false/competitive/cold',
                      'difference': '192 RGB channels: baseline63, independent reference64; alpha 255 in both'},
          'mode_summary': mode_summary, 'cells': cells, 'evidence': evidence,
          'validation': {'native_renderer_tests': 226, 'conformance_cases': 1249,
                         'wasi_compositor_tests': 104, 'wasi_pixel_budget_tests': 1,
                         'full_check': 'passed, jobs1'},
          'earlier_attempts': {
              'renderer-001': '220 passed, one private pixel-budget message assertion failed; final test checks both modes and unchanged pixel-before-decoded ordering',
              'focused-001': 'four initial precision tests passed before F32 fragment extension',
              'build-001-idle': 'runner rejected unsupported kind=build before launching; corrected to validation; no measured trial repeated'},
          'decision': 'Retain the independent accuracy correction and existing fragment source/cache semantics. No speed win, confidence-bound gate or competitor admission is claimed.',
          'limitations': plan['limitations'] + [
              'F32 chunks may repeat traversal/source evaluation and alter resource-limit outcomes on wide pages; unchanged caps are not memory or latency equivalence',
              'Native and WASI MaskSink layouts were not measured; measured whole-process RSS applies only to this matrix',
              'WASI execution validates correctness, not WASI speed or memory',
              'Accuracy modes retain distinct coverage, image/font and shading policies; this is not full unification',
              'Known-process polling is not OS-level exclusivity']}

out = ROOT / 'benchmarks/results/f32-preservation-cost.json'
out.write_text(json.dumps(result, indent=2) + '\n')
lines = [
    '# F32 preservation: retained accuracy improvement', '',
    'The renderer now keeps the planner-admitted F32 precision in Competitive mode. '
    'An F32 fragment-mask sink preserves cached glyph coverage and once-per-run source conversion. '
    'The known translucent-text error is corrected. This is an accuracy improvement with a descriptive cost diagnostic, not a new speed or release admission.', '',
    'The baseline is product commit `c70b4678`; the candidate is the F32 product overlay on `e45a3308`. '
    'Exact source and native binary hashes are in `f32-preservation-cost-bindings.json`. '
    'The parked color-allocation patch was absent from both measured arms.', '',
    'All 1,406 candidate outputs (preflight plus warmup/measured outputs) match the fixed independent RGB references with zero error, correct dimensions and alpha 255. '
    'The baseline has the same 37 known failures: 192 channels at 63 rather than 64 in Competitive text-knockout-false. '
    'Every other baseline output passes. This does not certify arbitrary PDFs or transparent output.', '',
    'The one frozen trial used 19 cases ×2 modes, five warmup and 31 measured pairs per cell, '
    'balanced randomized order (seed 20261110). All 2,736 run outputs and 76 preflight outputs were retained and independently audited. '
    'The metric is in-memory open+compile+retained-render time, excluding startup, file I/O, output emission and destruction. '
    'The cost run had 18 observer scans with no known competing job. It was not repeated.', '',
]
for mode, summary in mode_summary.items():
    lo, hi = summary['total_time_ratio_range']
    rlo, rhi = summary['median_rss_ratio_range']
    lines.append(f"- {mode.title()}: median of case time ratios **{summary['median_of_cell_total_time_ratios']:.4f}**; case range {lo:.4f}–{hi:.4f}. Median peak-RSS ratios range {rlo:.4f}–{rhi:.4f}.")
lines += ['', 'Ratios below 1 favor the candidate. These small variations do not establish a speed win; there are no confidence bounds or tail gate. '
          'The known-wrong baseline cell describes the cost of correcting accuracy, not equivalent-output superiority. '
          'Whole-process RSS includes code/startup and does not establish equal buffer layout or memory behavior on larger pages.', '',
          '| Case | Mode | Baseline µs | Candidate µs | Time ratio | Median RSS ratio | Max RSS ratio |',
          '|---|---|---:|---:|---:|---:|---:|']
for row in cells:
    case, mode, _ = row['cell']; b, c = (row['medians'][side] for side in ('baseline', 'candidate'))
    ratio = row['candidate_over_baseline']
    lines.append(f"| {case} | {mode} | {b['total_ns']/1000:.2f} | {c['total_ns']/1000:.2f} | {ratio['total_ns']:.4f} | {ratio['median_peak_rss_bytes']:.4f} | {ratio['maximum_peak_rss_bytes']:.4f} |")
lines += ['', 'Validation: the full `python3 -B tools/check.py --jobs 1` passed, including workspace tests, lint/docs, native/WASI host checks, 196 tooling tests and 1,249 conformance cases. '
          'The focused release renderer suite passed 226 tests. The same compiled WASI test artifact passed 104 compositor tests plus the pixel-before-decoded-budget test. '
          'Raw-bit mask parity, literal alpha recurrences, clipping order, colored backdrop/tile boundaries, cache work boundaries, source diagnostics and once-per-run function work are covered.', '',
          'The initial renderer attempt retained one failure because the direct private error message changed from page to surface after selecting the shared output allocator. '
          'The final test preserves the pixel 16 limit, adds decoded-bytes 0, and checks pixel admission first in both modes. No cap or independent pixel reference was relaxed. '
          'An initial build launch used an invalid runner kind and launched no build; its idle receipt is preserved. No measured trial was retried.', '',
          'Limits: the fixed matrix does not cover large dynamic/shading performance, browser or WASI timing. '
          'F32 chunks can repeat traversal and source evaluation on wide pages and consume different memory than byte bands. '
          'Native/WASI MaskSink layout was not measured. Coverage, image/font and shading policies still differ between modes. '
          'No claim of global perfection, full mode unification or closing the Hayro speed gap follows.', '',
          'Next separate performance slice: reserve the five rectangle path segments once, then measure incrementally on this retained stack. '
          'The color-component reuse patch remains available for a later independent experiment. Raw evidence is in `local/f32-preservation/`; '
          'the JSON companion retains all 38 cells, source identities and completed receipt/guard hashes.', '']
(ROOT / 'benchmarks/results/f32-preservation-cost.md').write_text('\n'.join(lines))
print(json.dumps({'status': result['status'], 'result': str(out), 'mode_summary': mode_summary}))
