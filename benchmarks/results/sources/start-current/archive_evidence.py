"""Bind final local records and published copies without altering measured data."""
import hashlib
import json
from pathlib import Path


def pin(path):
    digest = hashlib.sha256()
    with path.open('rb') as handle:
        for block in iter(lambda: handle.read(1 << 20), b''):
            digest.update(block)
    return {'path': str(path.resolve()), 'sha256': digest.hexdigest(),
            'bytes': path.stat().st_size}


root = Path.cwd().resolve()
results = root / 'benchmarks/results'
local = results / 'local/start-current'
audit = local / 'confirmation-003-audit.json'
expected = 'cbffe989ac870d059a0e467ac90247edcec6827f022506bef13747b2ddbb0f31'
assert pin(audit)['sha256'] == expected
record = json.loads(audit.read_bytes())
assert record['status'] == 'verified-confirmation'
assert record['attempted_processes'] == 34440
assert record['candidate_quality_verified'] and record['errors'] == []
for label in ('confirmation-003', 'confirmation-003-audit'):
    receipt = json.loads((local / (label + '-receipt.json')).read_bytes())
    guard = json.loads((local / (label + '-observer/guard.json')).read_bytes())
    assert receipt['status'] == guard['status'] == 'completed'
    assert receipt['exit_code'] == guard['exit_code'] == 0
    assert receipt['source_unchanged'] is True
    assert not receipt['errors'] and not receipt['artifact_errors']
    for path, digest in receipt['artifacts'].items():
        assert pin(Path(path))['sha256'] == digest
for original, published in (
    ('confirmation-003-audit.json', 'start-current.json'),
    ('preflight-001-audit.json', 'start-current-preflight.json'),
    ('accuracy-001-audit.json', 'start-current-accuracy.json'),
):
    assert (local / original).read_bytes() == (results / published).read_bytes()

metadata = set(local.glob('*.json'))
for name in ('report.json', 'plan.json', 'events.jsonl', 'inputs.json', 'guard.json'):
    metadata.update(local.glob('*/' + name))
for directory in local.glob('*-observer'):
    metadata.update(directory.glob('snapshot-*.txt'))
sources = {p for p in (results / 'sources/start-current').rglob('*')
           if p.is_file() and '__pycache__' not in p.parts}
published = {results / name for name in (
    'start-current.json', 'start-current-preflight.json', 'start-current-accuracy.json',
    'start-current.md', 'start-current-accuracy.md', 'start-current-cells.md',
    'start-current-bindings.json')}
published.add(root / 'docs/plans/start-to-current-measurement.md')
manifest = {
    'schema': 1, 'status': 'archived-verified-confirmation', 'admission': False,
    'confirmation': 'confirmation-003', 'audit_sha256': expected,
    'excluded_attempts': ['confirmation-001', 'confirmation-002'],
    'statistics_recomputed_by_this_script': False,
    'local_records': [pin(p) for p in sorted(metadata)],
    'archived_helper_sources': [pin(p) for p in sorted(sources)],
    'published_files': [pin(p) for p in sorted(published)],
    'limits': [
        'Raw outputs, logs and binaries remain local; final report/binding hashes bind them transitively.',
        'This manifest records already-audited evidence; it does not replace the independent audit.',
        'Interrupted attempts remain excluded; no samples or statistics are pooled.',
    ],
}
output = results / 'start-current-evidence.json'
with output.open('x') as handle:
    json.dump(manifest, handle, indent=2, allow_nan=False)
    handle.write('\n')
print(json.dumps(pin(output)))
