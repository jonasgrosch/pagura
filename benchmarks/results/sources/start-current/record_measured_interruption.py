"""Retain the externally canceled second trial without computing statistics."""
import hashlib
import json
from pathlib import Path

root = Path.cwd()
local = root / 'benchmarks/results/local/start-current'
directory = local / 'confirmation-002'
pin = lambda p: {'path': str(p), 'sha256': hashlib.sha256(p.read_bytes()).hexdigest()}
report_path = directory / 'report.json'
receipt_path = local / 'confirmation-002-receipt.json'
guard_path = local / 'confirmation-002-observer/guard.json'
report, receipt, guard = (json.loads(p.read_text()) for p in (report_path, receipt_path, guard_path))
assert report['status'] == 'cancelled' and receipt['status'] == 'failed'
assert receipt['exit_code'] == receipt['wrapper_exit_code'] == 143
assert receipt['source_unchanged'] and not receipt['errors'] and not receipt['artifact_errors']
assert guard['status'] == 'invalid-external-contention'
assert any(event['matches'] for event in guard['events'])
for path, digest in receipt['artifacts'].items():
    assert pin(Path(path))['sha256'] == digest
rows = report['records']
complete = [row for row in rows if 'finished_monotonic_ns' in row]
assert all(row['classification'] in ('both-correct-equal', 'baseline-incorrect-current-correct')
           for row in complete)
record = {
    'schema': 1, 'experiment': 'start-current', 'status': 'excluded-external-contention',
    'admission': False, 'report': pin(report_path), 'receipt': pin(receipt_path),
    'guard': pin(guard_path), 'plan': pin(directory / 'plan.json'),
    'journal': pin(directory / 'events.jsonl'),
    'pair_records_by_phase': {phase: sum(row['phase'] == phase for row in rows)
                              for phase in ('warmup', 'measured')},
    'completed_pairs_by_phase': {phase: sum(row['phase'] == phase for row in complete)
                                 for phase in ('warmup', 'measured')},
    'completed_arm_records': sum(label in row for row in rows for label in ('B', 'C')),
    'unfinished_pair_records': len(rows) - len(complete),
    'performance_statistics_computed': False,
    'quality_scope': 'Producer classifications only; this interrupted trial has not passed the independent completed-run audit.',
    'decision': 'Consume ordinal 002. Retain and exclude every sample; do not resume or pool. After the foreign work is terminal and a fresh quiet observation succeeds, start a complete ordinal 003 with unchanged source, binaries, inputs, schedule, counts, statistics and quality gates.',
    'reason': 'A new local Cargo check triggered strict cancellation during measured sampling. No timing statistics or confidence bounds were inspected. Recovery is based only on external invalidation, not a favorable or unfavorable performance result.'}
with (local / 'confirmation-002-stop.json').open('x') as handle:
    json.dump(record, handle, indent=2)
    handle.write('\n')
print(json.dumps({key: record[key] for key in ('status', 'pair_records_by_phase',
      'completed_pairs_by_phase', 'completed_arm_records', 'unfinished_pair_records')}))
