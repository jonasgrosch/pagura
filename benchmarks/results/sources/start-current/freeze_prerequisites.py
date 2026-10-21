"""Freeze successful independent preflight/accuracy audits before timing."""
import hashlib
import json
from pathlib import Path

root = Path.cwd()
local = root / 'benchmarks/results/local/start-current'
pin = lambda p: {'path': str(p), 'sha256': hashlib.sha256(p.read_bytes()).hexdigest()}
binding = root / 'benchmarks/results/start-current-bindings.json'
preflight = json.loads((local / 'preflight-001-audit.json').read_text())
accuracy = json.loads((local / 'accuracy-001-audit.json').read_text())
assert preflight['status'] == 'verified-preflight' and preflight['candidate_preflight_correct']
assert preflight['attempted_processes'] == 168 and preflight['quality_qualified_cells'] == 83
assert preflight['bindings_sha256'] == pin(binding)['sha256']
assert accuracy['status'] == 'verified-accuracy-probes' and accuracy['candidate_quality_verified']
assert accuracy['outputs_verified'] == 108
prerequisites = {'schema': 1, 'experiment': 'start-current', 'bindings_sha256': pin(binding)['sha256']}
for key, label, audit in (('preflight', 'preflight-001', preflight), ('accuracy', 'accuracy-001', accuracy)):
    report = local / label / 'report.json'
    assert audit['report_sha256'] == pin(report)['sha256']
    receipt = local / (label + '-receipt.json')
    guard = local / (label + '-observer/guard.json')
    data = json.loads(receipt.read_text())
    assert data['status'] == 'completed' and data['exit_code'] == data['wrapper_exit_code'] == 0
    assert data['source_unchanged'] and not data['errors'] and not data['artifact_errors']
    assert data['artifacts'][str(report)] == pin(report)['sha256']
    assert data['artifacts'][str(guard)] == pin(guard)['sha256']
    prerequisites[key] = {'report': pin(report), 'receipt': pin(receipt), 'guard': pin(guard),
                          'audit': pin(local / (label + '-audit.json'))}
    if key == 'preflight':
        prerequisites[key].update(plan=pin(local / label / 'plan.json'), journal=pin(local / label / 'events.jsonl'))
prerequisites['auditor_sources_sha256'] = {str(path): pin(path)['sha256'] for path in (
    root / 'benchmarks/results/sources/start-current/auditor/audit.py',
    root / 'benchmarks/results/sources/start-current/accuracy/audit.py')}
assert preflight['audit_source_sha256'] == prerequisites['auditor_sources_sha256'][str(root / 'benchmarks/results/sources/start-current/auditor/audit.py')]
output = local / 'preflight-pins.json'
with output.open('x') as handle:
    json.dump(prerequisites, handle, indent=2)
    handle.write('\n')
print(pin(output)['sha256'])
