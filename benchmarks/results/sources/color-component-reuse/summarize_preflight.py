"""Recheck all frozen quality outputs; preserve the intentionally failed gate."""
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[4]
sys.path[:0] = [str(ROOT / 'benchmarks'), str(ROOT / 'tools')]
import color_reuse as experiment
from paired_render import expected_image
from benchkit.images import normalize

local = ROOT / 'benchmarks/results/local/color-component-reuse'
directory = local / 'preflight-002'
sha = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
load = lambda path: json.loads(path.read_text())
report = load(directory / 'report.json')
receipt = load(local / 'preflight-002-receipt.json')
guard = load(local / 'preflight-002-observer/guard.json')
bindings_path = ROOT / 'benchmarks/results/color-reuse-bindings.json'
bindings = load(bindings_path)
assert report['status'] == 'blocked-quality' and not report['admission']
assert report['run_id'] == receipt['run_id']
assert Path(report['receipt']) == local / 'preflight-002-receipt.json'
assert receipt['status'] == 'failed' and receipt['source_unchanged']
assert receipt['source_before'] == receipt['source_after']
assert receipt['source_before']['sha256'] == bindings['candidate_runner_source_sha256']
assert receipt['exit_code'] == receipt['wrapper_exit_code'] == 1
assert not receipt['errors'] and not receipt['artifact_errors']
assert receipt['cancellation_signal'] is None
assert guard['status'] == 'child-failed' and guard['observation_only'] is False
assert guard['exit_code'] == guard['child_exit_code'] == 1
assert guard.get('cancellation_requested_utc') is None
assert all(event['matches'] == [] for event in guard['events'])
assert {'preflight', 'postflight'} <= {event['stage'] for event in guard['events']}
command = ['python3', '-B', 'benchmarks/preflight_color_reuse.py', '--bindings',
           'benchmarks/results/color-reuse-bindings.json', '--output',
           'benchmarks/results/local/color-component-reuse/preflight-002']
assert guard['command'] == command
assert receipt['argv'] == ['python3', '-B',
    'benchmarks/results/sources/analytic19-conversion/load_observer.py',
    '--evidence-dir', 'benchmarks/results/local/color-component-reuse/preflight-002-observer',
    '--', *command]
for path, digest in receipt['artifacts'].items():
    assert sha(Path(path)) == digest
assert report['identities'] == bindings['identities']
assert report['bindings_sha256'] == sha(bindings_path)
assert (directory / 'bindings.json').read_bytes() == bindings_path.read_bytes()
for name, digest in bindings['bound_files_sha256'].items():
    assert sha(ROOT / name) == digest, name
for identity in bindings['identities'].values():
    assert sha(Path(identity['binary'])) == identity['binary_sha256']
for name, artifact in report['artifacts'].items():
    path = (directory / name).resolve()
    assert path.is_relative_to(directory) and sha(path) == artifact['sha256']
    assert path.stat().st_size == artifact['bytes']
expected_order = [(case, mode, operation, side) for case, mode, operation in experiment.CELLS
                  for side in ('baseline', 'candidate')]
assert len(report['samples']) == len(expected_order) == 76
failures, pairs = [], {}
for sample, key in zip(report['samples'], expected_order):
    assert tuple(sample[name] for name in ('case', 'mode', 'operation', 'side')) == key
    case, mode, operation, side = key
    pdf, oracle = experiment.fixture(case)
    assert (directory / 'inputs' / (case + '.pdf')).read_bytes() == pdf
    assert (directory / 'oracles' / (case + '.ppm')).read_bytes() == oracle
    assert sample['command'] == [bindings['identities'][side]['binary'],
        str(directory / 'inputs' / (case + '.pdf')), mode, operation,
        str(directory / 'pending.rgba'), '72']
    assert sample['process']['exit_code'] == 0 and sample['process']['status'] != 'timeout'
    expected = expected_image(oracle, operation)
    raw = (directory / sample['output']).read_bytes()
    actual = normalize(raw)
    header = f'RGBA8 {expected[0]} {expected[1]} premultiplied\n'.encode()
    assert raw.startswith(header) and raw[len(header)+3::4] == b'\xff'*(expected[0]*expected[1])
    assert actual[:2] == expected[:2]
    errors = [abs(a-b) for a, b in zip(actual[2], expected[2])]
    assert sample['max_channel_error'] == max(errors, default=0)
    assert sample['different_channels'] == sum(value != 0 for value in errors)
    assert sample['oracle_equal'] == (actual == expected)
    assert sample['status'] == ('passed' if actual == expected else 'failed')
    pairs.setdefault((case, mode), {})[side] = raw
    if actual != expected:
        failures.append({'case':case, 'mode':mode, 'side':side,
            'max_channel_error':max(errors), 'different_channels':sum(v != 0 for v in errors),
            'different_values':sorted({(a, b) for a, b in zip(actual[2], expected[2]) if a != b})})
assert len(pairs) == 38 and all(pair['baseline'] == pair['candidate'] for pair in pairs.values())
assert not report['paired_output_differences']
assert len(failures) == len(report['failures']) == 2
assert {(item['case'], item['mode'], item['side']) for item in failures} == {
    ('paint/text-knockout-false', 'competitive', side) for side in ('baseline','candidate')}
summary = {'schema':1, 'status':'blocked-quality', 'admission':False,
    'quality_outputs_rechecked':76, 'independent_rgb_passes':74, 'opaque_alpha_passes':76,
    'identical_before_after_pairs':38, 'failures':failures,
    'timing_pilot_launched':False, 'confirmation_launched':False,
    'latency_gain_established':False, 'memory_gain_established':False,
    'known_foreign_jobs_observed':False, 'observer_scans':guard['scans'],
    'identities':bindings['identities'],
    'artifacts_sha256':{str(path.relative_to(ROOT)):sha(path) for path in
        (directory/'report.json', local/'preflight-002-receipt.json',
         local/'preflight-002-observer/guard.json', bindings_path,
         ROOT/'benchmarks/results/color-reuse-plan.json')},
    'prior_attempt':'preflight-001 stopped before any renderer invocation because bindings were absent after a coordinator assertion failed. Its failed receipt is retained; no samples were reused.',
    'decision':'Preserve the allocation candidate as a patch, without accepting an unmeasured speedup. Repair the preexisting Competitive composition loss separately; then rebase and freeze a fresh incremental experiment.',
    'scope':'Independent analytic19 RGB8 references, opaque white, 72 DPI, retained compilation. This is not the CLI image entrypoint or full-corpus perfection.'}
(ROOT/'benchmarks/results/color-reuse-preflight.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps({key:value for key,value in summary.items() if key not in ('identities','artifacts_sha256')},indent=2))
