"""Pin terminal collection evidence externally for the independent auditor."""
import hashlib
import json
from pathlib import Path
import sys

root = Path.cwd()
label = sys.argv[1]
if label not in ('preflight-001', 'confirmation-001', 'confirmation-002', 'confirmation-003'):
    raise ValueError('Unknown fixed collection')
local = root / 'benchmarks/results/local/start-current'
directory = local / label
pin = lambda p: {'path': str(p), 'sha256': hashlib.sha256(p.read_bytes()).hexdigest()}
receipt_path = local / (label + '-receipt.json')
guard_path = local / (label + '-observer/guard.json')
receipt, guard = (json.loads(p.read_text()) for p in (receipt_path, guard_path))
if receipt['status'] != 'completed' or guard['status'] != 'completed':
    raise ValueError('Collection is not terminal')
if receipt['source_unchanged'] is not True or receipt['errors'] or receipt['artifact_errors']:
    raise ValueError('Incomplete source/artifact evidence')
for path, digest in receipt['artifacts'].items():
    if pin(Path(path))['sha256'] != digest:
        raise ValueError('Completed artifact changed')
pins = {'schema': 1, 'experiment': 'start-current', 'root': str(root), 'phase': label.split('-')[0],
        'bindings': pin(root / 'benchmarks/results/start-current-bindings.json'),
        'report': pin(directory / 'report.json'), 'plan': pin(directory / 'plan.json'),
        'journal': pin(directory / 'events.jsonl'), 'receipt': pin(receipt_path), 'guard': pin(guard_path),
        'guard_script': pin(root / 'benchmarks/results/sources/analytic19-conversion/load_observer.py'),
        'guard_snapshots': [pin(p) for p in sorted(guard_path.parent.glob('snapshot-*.txt'))],
        'statistics': pin(root / 'tools/benchkit/statistics.py')}
if label.startswith('confirmation'):
    pins['prerequisites'] = pin(local / 'preflight-pins.json')
output = local / (label + '-audit-pins.json')
with output.open('x') as handle:
    json.dump(pins, handle, indent=2)
    handle.write('\n')
print(pin(output)['sha256'])
