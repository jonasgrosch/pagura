"""Publish the frozen u16 experiment after independent audits and review."""
import hashlib
import json
import re
from pathlib import Path

root = Path.cwd()
results = root / 'benchmarks/results'
local = results / 'local/n16-output-u16'
sha = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
read = lambda name: json.loads((local / name).read_text())
confirmation = read('confirmation-001-analysis.json')
pilot = read('pilot-001-analysis.json')
wasm = read('wasm-diagnostic-001-analysis.json')
decision = read('retention-decision.json')
bindings = json.loads((results / 'n16-output-u16-bindings.json').read_text())
assert confirmation['phase'] == 'confirmation'
assert confirmation['independently_verified_outputs'] == 18860
assert not confirmation['background_contention_observed']
assert decision['analysis_sha256'] == sha(local / 'confirmation-001-analysis.json')
assert decision['decision'] == 'retain-experimentally'
assert any(c['role'] == 'target' and c['latency']['median_upper'] <= .95
           for c in confirmation['cells'])
supported = [c for c in confirmation['cells'] if c['role'] == 'protected'
             and (c['latency']['median_lower'] > 1.02 or c['latency']['p95_lower'] > 1.02)]
assert decision['supported_protected_regressions'] == supported
precision = read('accuracy-001/report.json')
host = read('host-001/report.json')
assert precision['all_within_contract'] and precision['full_tile_equal']
assert len(precision['rows']) == 8
assert {(r['case'], r['mode'], r['operation']) for r in precision['rows']} == {
    (case, mode, operation) for case in ('root-two-partial', 'root-slivers-1000')
    for mode in ('exact', 'competitive') for operation in ('full', 'tile')}
assert all(r['within_contract'] and r['exit_code'] == 0 for r in precision['rows'])
assert host['status'] == 'passed' and len(host['cases']) == 2
assert {c['id'] for c in host['cases']} == {
    'content/precision-root-two-partial', 'content/precision-root-slivers-1000'}
assert all(c['status'] == 'passed' and c['raw_response_parity']
           and c['diagnostics'] == [] for c in host['cases'])
assert wasm['status'] == 'verified' and wasm['verified_outputs'] == 168
assert not wasm['suspension_cells'] and not wasm['background_contention_observed']
for key, path in [
    ('report_sha256', local / 'wasm-diagnostic-001/report.json'),
    ('receipt_sha256', local / 'wasm-diagnostic-001-receipt.json'),
    ('guard_sha256', local / 'wasm-diagnostic-001-observer/guard.json'),
    ('plan_sha256', local / 'wasm-diagnostic-001/plan.json'),
    ('build_bindings_sha256', local / 'wasm-build-bindings.json'),
    ('native_bindings_sha256', results / 'n16-output-u16-bindings.json'),
    ('auditor_sha256', local / 'wasm-diagnostic-src/audit.py'),
]:
    assert wasm[key] == sha(path), key
assert len(wasm['cells']) == 6
for c in wasm['cells']:
    memory = c['linear_memory_end_bytes']
    assert len(memory['baseline']) == 12
    assert all(type(v) is int and v > 0 for v in memory['baseline'])
    assert memory['baseline'] == memory['candidate']

for name, analysis in [('pilot-001', pilot), ('confirmation-001', confirmation)]:
    for key, path in [
        ('report_sha256', local / name / 'report.json'),
        ('receipt_sha256', local / (name + '-receipt.json')),
        ('guard_sha256', local / (name + '-observer/guard.json')),
        ('plan_sha256', results / 'n16-output-u16-plan.json'),
        ('bindings_sha256', results / 'n16-output-u16-bindings.json'),
    ]:
        assert analysis[key] == sha(path), (name, key)

for name in ('pilot-001', 'confirmation-001', 'wasm-diagnostic-001'):
    path = local / (name + '-analysis.json')
    assert read(name + '-audit-receipt.json')['artifacts'].get(str(path)) == sha(path)

def valid_receipt(receipt):
    return (receipt['status'] == 'completed' and receipt['source_unchanged'] is True
            and receipt['exit_code'] == receipt['wrapper_exit_code'] == 0
            and not receipt['errors'] and not receipt['artifact_errors']
            and receipt['source_before'] == receipt['source_after']
            and receipt['source_root'] == str(root)
            and receipt['source_before']['commit'] == bindings['identities']['candidate']['commit']
            and receipt['source_before']['sha256'] == bindings['candidate_runner_source_sha256'])

runs = ['full-check-001', 'accuracy-001', 'pilot-001', 'pilot-001-audit',
        'confirmation-001', 'confirmation-001-audit',
        'wasm-diagnostic-001', 'wasm-diagnostic-001-audit']
for name in runs:
    receipt = read(name + '-receipt.json')
    assert valid_receipt(receipt), name
    for path, digest in receipt['artifacts'].items():
        assert sha(Path(path)) == digest, (name, path)

files = [name + '-receipt.json' for name in runs] + [
    'full-check-001.log', 'full-check-001-observer/guard.json',
    'accuracy-001/report.json', 'host-001/report.json',
    'accuracy-001-observer/guard.json', 'pilot-001/report.json',
    'pilot-001-observer/guard.json', 'pilot-001-analysis.json',
    'confirmation-launch.json', 'confirmation-001/report.json',
    'confirmation-001-observer/guard.json', 'confirmation-001-analysis.json',
    'retention-decision.json', 'native-candidate.patch',
    'wasm-build-bindings.json', 'wasm-baseline-build-001-receipt.json',
    'wasm-candidate-build-001-receipt.json', 'wasm-diagnostic-001/report.json',
    'wasm-diagnostic-001-observer/guard.json', 'wasm-diagnostic-001-analysis.json',
    'wasm-audit-pins.json', 'disassembly-evidence.json',
    'build-001-receipt.json', 'build-002-receipt.json',
    'launch_confirmation.py', 'full_check.py', 'run_wasm_diagnostic.py',
    'run_wasm_audit.py', 'wasm-diagnostic-src/diagnostic.py',
    'wasm-diagnostic-src/run_profile_wasi.mjs', 'wasm-diagnostic-src/audit.py',
    'finalize.py',
]
log = (local / 'full-check-001.log').read_text()
assert 'Conformance: 1249 passed, 0 failed of 1249 cases' in log
assert 'Ran 196 tests' in log and 'All checks passed.' in log
rust_tests = sum(map(int, re.findall(r'test result: ok\. (\d+) passed;', log)))
assert rust_tests == 876
summary = dict(confirmation)
summary['identities'] = bindings['identities']
summary['retention_decision'] = decision
summary['pilot'] = {k: pilot[k] for k in (
    'independently_verified_outputs', 'target_latency_passes',
    'protected_latency_passes', 'memory_passes', 'report_sha256')}
summary['wasm_diagnostic'] = wasm
summary['full_check'] = {'all_checks_passed': True, 'rust_tests': rust_tests,
    'renderer_tests': 210, 'python_tests': 196, 'conformance_passed': 1249,
    'conformance_failed': 0,
    'duration_seconds': read('full-check-001-receipt.json')['duration_ns'] / 1e9}
summary['accuracy'] = {'independent_precision_outputs': 8,
    'native_wasi_default_exact_parity': True,
    'backgrounds': 'Existing independent colored-alpha 0/128/254/255 full/offset-tile regressions pass in both modes.',
    'quantizer': 'Native exhaustive test covers all 65,536 u16 inputs, nonzero source offset, incomplete-pixel tails and excess destination capacity.'}
summary['limitations'] = [
    'Incremental native synthetic workload result against f98ad836, not a net legacy-stack or competitor comparison.',
    'Measured dirty source on f98ad836 is pinned by full product fingerprints and hash-bound binaries before later commit.',
    'Baseline native binary is reused from the previous guarded experiment with matching product fingerprint; not independently rebuilt.',
    'No historical or pilot samples are pooled into confirmation.',
    'Bootstrap bounds come from the frozen harness and are not independently recomputed. Bounds are marginal; 200 pairs do not establish calibrated p95 or simultaneous coverage.',
    'Peak RSS is the fresh renderer child OS high-water value across the whole process; unchanged explicit storage does not erase observed misses.',
    'WASI fresh-process diagnostic excludes startup/explicit compile/instantiate; lazy tiering may occur inside render. Linear memory is not total memory or RSS.',
    'Opaque-background timing matrix. Earlier nonopaque-background F64 cost and original shared-composition speed/RSS costs remain open.',
    'Known-process observation can miss short or unrecognized jobs. No controlled-runner certification, holdout or worldwide superiority evidence.',
]
aa_path = local / 'rss-aa-001/report.json'
if aa_path.exists():
    aa = json.loads(aa_path.read_text())
    aa_receipt = read('rss-aa-001-receipt.json')
    assert aa_receipt['artifacts'].get(str(aa_path)) == sha(aa_path)
    aa_verified = valid_receipt(aa_receipt) and aa['status'] == 'completed-diagnostic'
    if aa_verified:
        assert aa['verified_processes'] == 84 and aa['admission'] is False
        assert aa['thresholds_changed'] is False
        assert aa['binary_sha256'] == bindings['identities']['candidate']['binary_sha256']
        for path, digest in aa_receipt['artifacts'].items():
            assert sha(Path(path)) == digest
        aa_audit = read('rss-aa-001-analysis.json')
        audit_receipt = read('rss-aa-002-audit-receipt.json')
        assert valid_receipt(audit_receipt)
        assert audit_receipt['artifacts'][str(local / 'rss-aa-001-analysis.json')] == sha(local / 'rss-aa-001-analysis.json')
        assert aa_audit['status'] == 'verified-evidence' and aa_audit['verified_processes'] == 84
        assert aa_audit['prior_failed_gates_unchanged'] is True
        assert not aa_audit['background_contention_observed']
        for key, path in [('report_sha256', aa_path),
                          ('receipt_sha256', local / 'rss-aa-001-receipt.json'),
                          ('guard_sha256', local / 'rss-aa-001-observer/guard.json'),
                          ('auditor_sha256', local / 'rss-aa-src/audit-v2.py')]:
            assert aa_audit[key] == sha(path)
        assert aa_audit['summaries'] == aa['summaries']
    summary['rss_aa_diagnostic'] = {key: aa.get(key) for key in (
        'status', 'admission', 'thresholds_changed', 'verified_processes',
        'summaries', 'plan_sha256', 'binary_sha256', 'limitations')}
    summary['rss_aa_diagnostic']['verified_completed_run'] = aa_verified
    summary['rss_aa_diagnostic']['interpretation'] = (
        'Descriptive same-binary observations only; no prior gate or regression is erased.' if aa_verified
        else 'Incomplete or unverified evidence; cannot interpret performance or RSS failures.')
    files += ['rss-aa-001/report.json', 'rss-aa-001-receipt.json',
              'rss-aa-001-observer/guard.json', 'rss-aa-src/rss_aa.py',
              'rss-aa-src/test_rss_aa.py', 'run_rss_aa.py', 'rss-aa-tests-001.log',
              'rss-aa-001-analysis.json', 'rss-aa-001-audit-receipt.json',
              'rss-aa-002-audit-receipt.json', 'rss-aa-src/audit.py',
              'rss-aa-src/audit-v2.py', 'rss-aa-audit-pins.json',
              'run_rss_aa_audit.py', 'run_rss_aa_audit_v2.py']
summary['evidence_files_sha256'] = {
    str((local / name).relative_to(results)): sha(local / name) for name in files}
with (results / 'n16-output-u16-experiment.json').open('x') as output:
    json.dump(summary, output, indent=2)
    output.write('\n')

lines = ['# Exact N16 output in 16-bit lanes', '',
    '2026-10-21. Retained as an experimental partial win against `f98ad836`, keeping the glyph memo, clip work, shared opaque accuracy correction and background guard in both arms.', '',
    'The only product change uses saturating u16 addition in the existing exact shift/subtract converter. Above the saturation boundary both formulas return 255; below it they are identical. It removes a lint exception and adds no storage, cache, dispatch, resource charge or accuracy-mode change.', '',
    'The fixed confirmation ran 200 fresh pairs and five warmups for each of 46 cells. All 18,860 outputs passed independent RGB, opaque-alpha and same-mode byte checks. No pilot or historical samples enter these estimates.', '',
    '| Target | Baseline median ms | Candidate median ms | Median ratio (upper) | p95 ratio (upper) | Peak RSS MiB baseline → candidate |',
    '|---|---:|---:|---:|---:|---:|']
for c in sorted((c for c in confirmation['cells'] if c['role'] == 'target'), key=lambda c:c['mode'], reverse=True):
    latency, rss = c['latency'], c['peak_rss_mib']
    lines.append(f"| {c['case']} / {c['mode']} / {c['operation']} | {c['baseline_median_ms']:.6f} | {c['candidate_median_ms']:.6f} | {latency['median_ratio']:.4f} ({latency['median_upper']:.4f}) | {latency['p95_ratio']:.4f} ({latency['p95_upper']:.4f}) | {rss['baseline']:.6f} → {rss['candidate']:.6f} |")
lines += ['', f"Target latency gates: {confirmation['target_latency_passes']}/2; protected latency gates: {confirmation['protected_latency_passes']}/44; peak-RSS gates: {confirmation['memory_passes']}/46. **Full admission remains false.** Every failed gate remains visible below and in the complete JSON.", '',
    'Ratios are candidate/baseline. Upper bounds are nominal one-sided 95% paired-bootstrap bounds from the pinned harness. They do not establish simultaneous coverage across the matrix or guarantee p95 calibration. The independent auditor recomputes point estimates and verifies artifacts; it does not recompute bootstrap bounds.', '',
    'Full-page latency covers render after open/compile and excludes input reads and output emission. Cold cells include open/compile but do not measure cold filesystem I/O. RSS covers the whole fresh process. No known competing jobs were observed; this is not controlled-runner certification.', '',
    '## Accuracy and validation', '',
    'Full repository validation passed: 876 Rust tests including 210 renderer tests, 196 Python tests, native/WASM and host checks, and 1,249/1,249 conformance cases. The native exhaustive 65,536-word converter test passes, including nonzero source offset, incomplete-pixel tails and excess destination capacity. Independent precision full/tile probes pass both modes under the stated coverage-byte composition model; default Exact host output matches native/WASI. Existing colored-background tests also pass. This does not prove arbitrary-PDF or continuous-geometry correctness.', '',
    'The first focused build stopped on the obsolete lint expectation; its failure is retained. The corrected build and full check passed. This has no relation to selecting performance samples.', '',
    '## WASM diagnostic', '',
    'A separately frozen 12-pair, six-cell Node/WASI screen verified 168 outputs against independently reconstructed analytic pixels. The predeclared material-regression signal did not fire. Every per-cell linear-memory end size matched between sides. This establishes no WASM speed, warmed-browser, total-memory or RSS admission.', '',
    '| Case / mode | Median ratio | p95 ratio | Linear memory bytes baseline / candidate |',
    '|---|---:|---:|---:|']
for c in wasm['cells']:
    memory = c['linear_memory_end_bytes']
    lines.append(f"| {c['case']} / {c['mode']} | {c['median_ratio']:.4f} | {c['p95_ratio']:.4f} | {sorted(set(memory['baseline']))} / {sorted(set(memory['candidate']))} |")
lines += ['', 'Explicit module compile/instantiate and Node startup are outside the internal render timer; lazy compilation and tiering may remain inside its first render. Module hashes, identical Cargo commands, runtime pins, raw samples and independent audit are retained. ARM disassembly shows saturating 16-bit vector addition and narrowing subtraction; that is instruction evidence, not a portable speed claim.', '',
    '## Same-binary memory diagnostic', '',
    'A separate fixed diagnostic ran the same hash-bound native binary for both labels: two warmup pairs and twelve measured pairs in three Exact/full cells. All 84 outputs, raw Darwin RSS values and summaries passed independent audit. The collector already uses per-child OS high-water RSS in bytes; it is not a periodic sampler or cumulative process counter. Three pure diagnostic tests pass.', '',
    '| Case | Median RSS MiB A / B | Maximum RSS MiB A / B | Maximum A/B ratio |',
    '|---|---:|---:|---:|']
for case, row in summary['rss_aa_diagnostic']['summaries'].items():
    sides = row['rss_bytes_by_side']
    lines.append(f"| {case} | {sides['A']['median']/2**20:.6f} / {sides['B']['median']/2**20:.6f} | {sides['A']['maximum']/2**20:.6f} / {sides['B']['maximum']/2**20:.6f} | {row['maximum_ratios']['A_over_B']:.4f} |")
lines += ['', 'Identical code produces appreciable peak variation, including a 46.4% difference between the default-knockout maxima despite equal medians. This small descriptive run neither establishes the cause of prior failures nor subtracts a noise allowance. Every prior gate remains unchanged. Its first audit attempt rejected the macOS platform string because the auditor incorrectly expected Darwin; the corrected auditor verifies the same retained samples. The failed audit and original source remain preserved; no renderer sample was repeated.', '',
    '## Every native confirmation cell', '',
    '| Case / mode / operation | Median ratio | p95 ratio | Peak RSS ratio | Latency gate | RSS gate |',
    '|---|---:|---:|---:|---|---|']
for c in sorted(confirmation['cells'], key=lambda c:(c['case'],c['mode'],c['operation'])):
    lines.append(f"| {c['case']} / {c['mode']} / {c['operation']} | {c['latency']['median_ratio']:.4f} | {c['latency']['p95_ratio']:.4f} | {c['peak_rss_ratio']:.4f} | {'pass' if c['median_pass'] and c['p95_pass'] else 'miss'} | {'pass' if c['memory_pass'] else 'miss'} |")
lines += ['', '## Retention and limits', '', decision['restriction'], '',
    'The five-pair pilot is separate: 920 verified outputs, 1/2 target latency, 13/44 protected latency and 34/46 RSS gates passed. Both target RSS cells passed in that pilot. Its Competitive target upper median bound missed the 5% benefit threshold, and two protected tail lower bounds exceeded 1.02; those controls stayed in confirmation.', '',
    '[Complete audited results and evidence hashes](n16-output-u16-experiment.json), [frozen native plan](n16-output-u16-plan.json), [source/binary bindings](n16-output-u16-bindings.json), [separate pilot](n16-output-u16-pilot.md). Native baseline reuses the prior guarded candidate binary whose product fingerprint matches committed `f98ad836`; the WASM baseline was rebuilt independently from its pristine control checkout.', '',
    'Historical percentages are not added together. The earlier opaque accuracy correction still has measured costs versus its legacy route, and nonopaque backgrounds still use the corrected, more expensive F64 path. A direct net-stack and fair competitor comparison remains required. The standing goal remains active.']
with (results / 'n16-output-u16-experiment.md').open('x') as output:
    output.write('\n'.join(lines) + '\n')
print(json.dumps({k: confirmation[k] for k in (
    'target_latency_passes', 'protected_latency_passes', 'memory_passes')}))
