"""Run one fixed command with the existing owned-child lifecycle and retain logs."""
import hashlib
import json
import os
from pathlib import Path
import sys

ROOT = Path.cwd()
sys.path.insert(0, str(ROOT / 'tools'))
from benchkit.config import source_digest
from benchkit.measurement import run_process

label, *command = sys.argv[1:]
local = ROOT / 'benchmarks/results/local/rectangle-reserve'
report_path = local / (label + '.json')
log_path = local / (label + '.log')
if report_path.exists() or log_path.exists():
    raise ValueError('Refusing to overwrite validation evidence')
before = source_digest()
script_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
os.environ.update(CARGO_BUILD_JOBS='1', RUST_TEST_THREADS='1', PYTHONDONTWRITEBYTECODE='1')
record = {'command': command, 'source_before': before, 'driver_sha256': script_hash,
          'status': 'incomplete', 'run_id': os.environ.get('PAGURA_RUN_ID')}
try:
    process, log = run_process(command, {'timeout_seconds': 1200, 'measure_rss': False},
                               log_budget=4 * 1024 * 1024)
    log_path.write_text(log)
    record['process'] = process
    record['log_sha256'] = hashlib.sha256(log_path.read_bytes()).hexdigest()
    if process['exit_code'] != 0 or process['status'] == 'timeout':
        raise RuntimeError('Validation command failed; retained log: ' + str(log_path))
    record['status'] = 'command-passed'
finally:
    record['source_after'] = source_digest()
    record['source_unchanged'] = before == record['source_after']
    report_path.write_text(json.dumps(record, indent=2) + '\n')
if not record['source_unchanged']:
    raise RuntimeError('Source changed during validation')
print(label + ': ' + record['status'], flush=True)
