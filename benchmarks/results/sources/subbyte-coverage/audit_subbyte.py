"""Independent arithmetic and artifact audit; no renderer or producer imports."""
from fractions import Fraction as F
import hashlib
import json
from pathlib import Path

root = Path.cwd()
local = root / 'benchmarks/results/local/n16-output-u16'
sha = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
load = lambda p: json.loads(p.read_text())
pins = load(local / 'subbyte-audit-pins.json')
for name, digest in pins.items():
    assert sha(local / name) == digest
directory = local / 'subbyte-001'
report = load(directory / 'report.json')
receipt = load(local / 'subbyte-001-receipt.json')
guard = load(local / 'subbyte-001-observer/guard.json')
bindings = load(root / 'benchmarks/results/n16-output-u16-bindings.json')
assert receipt['status'] == 'completed' and receipt['source_unchanged']
assert receipt['exit_code'] == receipt['wrapper_exit_code'] == 0
assert not receipt['errors'] and not receipt['artifact_errors']
assert receipt['source_before'] == receipt['source_after']
assert receipt['source_before']['sha256'] == bindings['candidate_runner_source_sha256']
assert receipt['source_before']['commit'] == bindings['identities']['candidate']['commit']
assert receipt['source_root'] == receipt['cwd'] == str(root)
assert report['runner']['coordination']['run_id'] == receipt['run_id']
for path, digest in receipt['artifacts'].items():
    assert sha(Path(path)) == digest
assert receipt['artifacts'][str(directory / 'report.json')] == pins['subbyte-001/report.json']
assert guard['status'] == 'completed-observation-only' and guard['exit_code'] == 0
assert not any(e['matches'] for e in guard['events'])
assert report['status'] == 'completed-diagnostic' and report['verified_outputs'] == 12
assert report['performance_admission'] is False and report['prior_gates_changed'] is False
assert report['normative_pdf_violation_claim'] is False
for name, record in report['retained_files'].items():
    assert sha(directory / name) == record['sha256']
    assert (directory / name).stat().st_size == record['bytes']
plan = load(directory / 'plan.json')
assert sha(directory / 'plan.json') == report['plan_sha256']
for name, digest in plan['bound_files_sha256'].items():
    assert sha(Path(name)) == digest
identity = bindings['identities']['candidate']
assert sha(Path(identity['binary'])) == identity['binary_sha256'] == plan['binary_sha256']
q = F(1, 1024)
small = 255 * (1 - q)
repeated = 255 * (1 - q) ** 1024
references = {
    'single-tiny': (F(255), small, small),
    'coincident-tiny': (F(255), repeated, small),
    'disjoint-tiny': (F(255), repeated, F(0)),
    'compound-tiny': (F(0), F(0), F(0)),
    'full-pixel': (F(0), F(0), F(0)),
    'disjoint-halves': (F(16129, 255), F(255, 4), F(0)),
}
names = ('declaredcoveragebyte', 'unquantizedpermark', 'finalsceneboxfilter')
expected = {'single-tiny': 255, 'coincident-tiny': 255, 'disjoint-tiny': 255,
            'compound-tiny': 0, 'full-pixel': 0, 'disjoint-halves': 63}
assert {(r['case'], r['mode']) for r in report['records']} == {
    (case, mode) for case in references for mode in ('exact', 'competitive')}
rows = []
for row in report['records']:
    case = row['case']
    rgba = (directory / row['output']).read_bytes()
    assert rgba == b'RGBA8 1 1 premultiplied\n' + bytes([expected[case]] * 3 + [255])
    assert row['rgba'] == list(rgba[-4:]) and row['verified'] and row['declared_gate_pass']
    for name, reference in zip(names, references[case]):
        declared = plan['inputs'][case]['references'][name]
        assert F(int(declared['numerator']), int(declared['denominator'])) == reference
        for error in row['reference_signed_errors'][name]:
            assert F(int(error['numerator']), int(error['denominator'])) == expected[case] - reference
    assert abs(expected[case] - references[case][0]) <= 1
    rows.append({'case': case, 'mode': row['mode'], 'rgba': row['rgba'],
                 'declared_model_pass': True,
                 'reference_decimal_display_only': {
                     name: plan['inputs'][case]['references'][name]['decimal_display_only'] for name in names}})
result = {'schema': 1, 'status': 'verified-evidence', 'verified_outputs': 12,
          'current_model_checks_pass': True, 'prior_gates_changed': False,
          'performance_admission': False, 'background_contention_observed': False,
          'rows': rows, 'evidence_sha256': pins, 'plan_sha256': report['plan_sha256'],
          'auditor_sha256': sha(Path(__file__)),
          'limitations': [
              'Independent exact arithmetic and retained pixels; PDF construction trusts statically reviewed pinned source.',
              'Predicted coverage bytes are not internal instrumentation; a mismatch would not localize geometry versus composition.',
              'Research references are separate models, not a normative PDF verdict or a retroactive u16 admission gate.']}
with (local / 'subbyte-001-analysis.json').open('x') as output:
    json.dump(result, output, indent=2)
    output.write('\n')
print('Verified all 12 raw outputs and three exact reference models; prior gates unchanged.')
