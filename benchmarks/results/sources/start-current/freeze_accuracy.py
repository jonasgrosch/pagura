"""Freeze independent accuracy probes against the completed off-tree builds."""
import hashlib
import json
from pathlib import Path
import sys

ROOT = Path.cwd()
HARNESS = Path('/private/tmp/pagura-start-current/accuracy')
sys.path[:0] = [str(HARNESS), str(ROOT / 'benchmarks')]
import references
from paired_render import source_identity

LOCAL = ROOT / 'benchmarks/results/local/start-current'
sha = lambda p: hashlib.sha256(Path(p).read_bytes()).hexdigest()
pin = lambda p: {'path': str(p), 'sha256': sha(p)}
build_path = LOCAL / 'accuracy-build-001/report.json'
receipt_path = LOCAL / 'accuracy-build-001-receipt.json'
guard_path = LOCAL / 'accuracy-build-001-observer/guard.json'
build, receipt, guard = (json.loads(p.read_text()) for p in (build_path, receipt_path, guard_path))
assert build['status'] == 'built-both-arms'
assert receipt['status'] == guard['status'] == 'completed'
assert receipt['source_unchanged'] and receipt['exit_code'] == receipt['wrapper_exit_code'] == 0
assert not receipt['errors'] and not receipt['artifact_errors']
assert guard['observation_only'] is False and guard.get('cancellation_requested_utc') is None
assert all(not event['matches'] for event in guard['events'])
assert receipt['run_id'] == build['run_id']
for path, digest in receipt['artifacts'].items():
    assert sha(path) == digest
assert receipt['artifacts'][str(build_path)] == sha(build_path)
assert receipt['artifacts'][str(guard_path)] == sha(guard_path)
for identity in build['identities'].values():
    assert source_identity(identity['root']) == {k: identity[k] for k in ('root', 'commit', 'source_sha256')}
    assert sha(identity['binary']) == identity['binary_sha256']
for path, digest in build['harness_sha256'].items():
    assert sha(path) == digest
driver = ROOT / 'benchmarks/results/sources/start-current/build_accuracy.py'
assert sha(driver) == build['driver_sha256']
paths = [HARNESS / 'collect.py', HARNESS / 'references.py', driver,
         ROOT / 'benchmarks/results/sources/start-current/freeze_accuracy.py',
         ROOT / 'benchmarks/paired_render.py', Path(sys.executable).resolve(), Path(sys.orig_argv[0]).resolve()]
harness = {**build['harness_sha256'], **{str(path): sha(path) for path in paths}}
bindings = {'schema': 1, 'experiment': 'start-current-accuracy', 'plan': references.PLAN,
            'identities': build['identities'], 'harness_sha256': harness,
            'completed_evidence': [pin(p) for p in (build_path, receipt_path, guard_path)]}
output = LOCAL / 'accuracy-bindings.json'
with output.open('x') as handle:
    json.dump(bindings, handle, indent=2)
    handle.write('\n')
print(sha(output))
