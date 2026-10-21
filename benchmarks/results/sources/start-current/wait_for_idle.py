"""Wait without taking the shared lock until known foreign compute is quiet."""
import argparse
import importlib.util
import json
from pathlib import Path
import sys
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', type=Path,
                    default=Path('benchmarks/results/local/start-current/idle-wait-001.json'))
args = parser.parse_args()
root = Path.cwd()
source = root / 'benchmarks/results/sources/analytic19-conversion/load_observer.py'
spec = importlib.util.spec_from_file_location('retained_observer', source)
observer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(observer)
output = root / args.output
if output.exists():
    raise ValueError('Idle-wait evidence already exists')
record = {'status': 'waiting', 'quiet_seconds_required': 45, 'poll_seconds': 5,
          'started_utc': observer.utc_now(), 'events': [],
          'scope': 'Known-process observation only; no shared lock held and no benchmark launched'}
deadline = time.monotonic() + 1800
quiet_since = None
last = None
try:
    while time.monotonic() < deadline:
        _, rows = observer.snapshot()
        excluded = observer.excluded_processes(rows, None)
        matches = [{'pid': pid, 'ppid': row['ppid'], 'started_local': row['started_local'], 'reason': reason}
                   for pid, row in rows.items() if pid not in excluded and (reason := observer.prohibited(row['command']))]
        now = time.monotonic()
        quiet_since = None if matches else (now if quiet_since is None else quiet_since)
        state = [(m['pid'], m['reason']) for m in matches]
        if state != last:
            record['events'].append({'utc': observer.utc_now(), 'matches': matches})
            output.write_text(json.dumps(record, indent=2) + '\n')
            print('known foreign jobs: ' + str(len(matches)), flush=True)
            last = state
        if quiet_since is not None and now - quiet_since >= 45:
            record.update(status='known-processes-quiet', finished_utc=observer.utc_now(), quiet_seconds=now-quiet_since)
            break
        time.sleep(5)
    else:
        record.update(status='wait-expired', finished_utc=observer.utc_now())
finally:
    output.write_text(json.dumps(record, indent=2) + '\n')
if record['status'] != 'known-processes-quiet':
    sys.exit(75)
print('45 seconds without known foreign compute; benchmark still needs its own strict guard', flush=True)
