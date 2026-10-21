"""Bind the completed builds and independently frozen inputs before preflight."""
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path.cwd()
sys.path.insert(0, str(ROOT / 'benchmarks'))
from paired_render import source_identity
from benchkit.config import source_digest

LOCAL = ROOT / 'benchmarks/results/local/start-current'
sha = lambda p: hashlib.sha256(Path(p).read_bytes()).hexdigest()
pin = lambda p: {'path': str(p), 'sha256': sha(p)}
report_path = LOCAL / 'build-001/report.json'
report = json.loads(report_path.read_text())
receipt_path = LOCAL / 'build-001-receipt.json'
receipt = json.loads(receipt_path.read_text())
guard_path = LOCAL / 'build-001-observer/guard.json'
guard = json.loads(guard_path.read_text())
assert report['status'] == 'built-both-arms'
assert receipt['status'] == guard['status'] == 'completed'
assert receipt['source_unchanged'] and receipt['exit_code'] == receipt['wrapper_exit_code'] == 0
assert not receipt['errors'] and not receipt['artifact_errors']
assert guard['observation_only'] is False and guard.get('cancellation_requested_utc') is None
assert all(not event['matches'] for event in guard['events'])
assert receipt['run_id'] == report['run_id']
assert receipt['artifacts'][str(report_path)] == sha(report_path)
assert receipt['artifacts'][str(guard_path)] == sha(guard_path)
for path, digest in receipt['artifacts'].items():
    assert sha(path) == digest
for identity in report['identities'].values():
    assert source_identity(identity['root']) == {k: identity[k] for k in ('root', 'commit', 'source_sha256')}
    assert sha(identity['binary']) == identity['binary_sha256']
files = list((ROOT / 'benchmarks/results/sources/start-current/collector').glob('*.py'))
files += [ROOT / path for path in (
    'benchmarks/results/sources/start-current/build.py',
    'benchmarks/results/sources/start-current/freeze_bindings.py',
    'benchmarks/results/sources/analytic19-conversion/load_observer.py',
    'benchmarks/opaque_composition.py', 'benchmarks/glyph_memo.py',
    'benchmarks/glyph_memo_fixtures.py', 'benchmarks/scene_index.py',
    'benchmarks/clip_viewport_fixtures.py', 'benchmarks/color_reuse_fixtures.py',
    'benchmarks/paired_render.py', 'tools/benchkit/measurement.py',
    'tools/benchkit/statistics.py', 'tools/runner.py',
    'crates/pagura-render/examples/scene_index_profile.rs',
    'benchmarks/results/sources/analytic19-conversion/manifest.json',
    'benchmarks/results/sources/analytic19-conversion/references.json')]
bindings = {'schema': 1, 'experiment': 'start-current', 'identities': report['identities'],
            'candidate_runner_source_sha256': source_digest(), 'build_report': pin(report_path),
            'completed_evidence': [{'receipt': pin(receipt_path), 'guard': pin(guard_path), 'artifacts': receipt['artifacts']}],
            'inputs': pin(LOCAL / 'frozen-inputs-001/inputs.json'),
            'bound_files_sha256': {str(path.relative_to(ROOT)): sha(path) for path in sorted(files)}}
output = ROOT / 'benchmarks/results/start-current-bindings.json'
with output.open('x') as handle:
    json.dump(bindings, handle, indent=2)
    handle.write('\n')
print(sha(output))
