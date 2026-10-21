"""Pin completed strict accuracy collection for a separate raw-output audit."""
import hashlib
import json
from pathlib import Path

root = Path.cwd()
local = root / 'benchmarks/results/local/start-current'
pin = lambda p: {'path': str(p), 'sha256': hashlib.sha256(p.read_bytes()).hexdigest()}
report_path = local / 'accuracy-001/report.json'
bindings_path = local / 'accuracy-bindings.json'
receipt_path = local / 'accuracy-001-receipt.json'
guard_path = local / 'accuracy-001-observer/guard.json'
report, bindings, receipt, guard = (json.loads(p.read_text()) for p in (report_path, bindings_path, receipt_path, guard_path))
assert receipt['status'] == guard['status'] == 'completed'
assert receipt['source_unchanged'] and receipt['exit_code'] == receipt['wrapper_exit_code'] == 0
assert not receipt['errors'] and not receipt['artifact_errors']
for path, digest in receipt['artifacts'].items():
    assert pin(Path(path))['sha256'] == digest
runtime = {str(Path(path).resolve()): pin(Path(path).resolve())['sha256']
           for path in (guard['command'][0], report['producer_argv'][0])}
assert all(bindings['harness_sha256'][path] == digest for path, digest in runtime.items())
pins = {'root': str(root), 'report': pin(report_path), 'bindings': pin(bindings_path),
        'receipt': pin(receipt_path), 'guard': pin(guard_path),
        'reviewed_build_evidence': bindings['completed_evidence'], 'command': guard['command'],
        'producer_argv': report['producer_argv'], 'runtime_files_sha256': runtime,
        'coordination_sources': {str(path): pin(path)['sha256'] for path in (
            root / 'tools/runner.py', root / 'benchmarks/results/sources/analytic19-conversion/load_observer.py')}}
output = local / 'accuracy-001-audit-pins.json'
with output.open('x') as handle:
    json.dump(pins, handle, indent=2)
    handle.write('\n')
print(pin(output)['sha256'])
