"""Retain a warmup-only externally interrupted trial without computing ratios."""
import hashlib
import json
from pathlib import Path

root = Path.cwd()
local = root / 'benchmarks/results/local/start-current'
pin = lambda p: {'path': str(p), 'sha256': hashlib.sha256(p.read_bytes()).hexdigest()}
report_path = local / 'confirmation-001/report.json'
receipt_path = local / 'confirmation-001-receipt.json'
guard_path = local / 'confirmation-001-observer/guard.json'
report, receipt, guard = (json.loads(p.read_text()) for p in (report_path, receipt_path, guard_path))
assert guard['status'] == 'invalid-external-contention'
assert report['status'] == 'cancelled' and receipt['status'] == 'failed'
assert receipt['exit_code'] == receipt['wrapper_exit_code'] == 143
assert receipt['source_unchanged'] and not receipt['artifact_errors']
assert any(event['matches'] for event in guard['events'])
assert all(row['phase'] == 'warmup' for row in report['records'])
for path, digest in receipt['artifacts'].items():
    assert pin(Path(path))['sha256'] == digest
record = {'schema': 1, 'experiment': 'start-current', 'status': 'excluded-external-contention',
          'admission': False, 'report': pin(report_path), 'receipt': pin(receipt_path), 'guard': pin(guard_path),
          'plan': pin(local / 'confirmation-001/plan.json'), 'journal': pin(local / 'confirmation-001/events.jsonl'),
          'warmup_pair_records': len(report['records']),
          'arm_records': sum(label in row for row in report['records'] for label in ('B', 'C')),
          'measured_pairs_started': 0, 'performance_statistics_computed': False,
          'decision': 'Consume ordinal 001. Do not resume, pool or qualify any of its samples. After terminal foreign work and a clean quiet check, launch one separate complete ordinal 002 with unchanged source, binaries, inputs, schedule, counts, statistics and quality gates.',
          'reason': 'Strict observer found an unrelated local Cargo check during warmups and canceled the owned collector. This is recovery from external invalidation before measured sampling, not a repeated trial selected by speed or confidence bounds.'}
with (local / 'confirmation-001-stop.json').open('x') as handle:
    json.dump(record, handle, indent=2)
    handle.write('\n')
print('Preserved interrupted warmups; no measured pairs or ratios')
