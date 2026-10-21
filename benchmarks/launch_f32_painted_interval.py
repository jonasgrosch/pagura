#!/usr/bin/env python3
"""Validate one native phase, record exclusive intent, then exec the fixed driver."""
import argparse
import json
import os
from pathlib import Path
import sys
import time

sys.dont_write_bytecode = True
from f32_interval_admission import (LOCAL, NAME, ROOT, decode, descriptor, driver_command,
                                    read, require, verify_config)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--config-sha256', required=True)
    parser.add_argument('--phase', choices=('pilot', 'confirmation'), required=True)
    parser.add_argument('--dry-run', action='store_true')
    args = parser.parse_args()
    config_path = args.config.resolve()
    config, bindings = verify_config(config_path, args.config_sha256, args.phase)
    command = driver_command(bindings, args.phase)
    run = args.phase + '-001'
    intent, start, output = LOCAL / (run + '-launch.json'), LOCAL / (run + '-driver-start.json'), LOCAL / run
    require(not any(p.exists() for p in (intent, start, output)), 'Phase already attempted; no automatic retry')
    if args.dry_run:
        print(json.dumps({'command': command, 'intent': str(intent), 'launches_nothing': True}, indent=2))
        return
    run_id, receipt_path = os.environ.get('PAGURA_RUN_ID'), os.environ.get('PAGURA_RUN_RECEIPT')
    require(run_id and receipt_path, 'Owned runner required')
    live = decode(read(Path(receipt_path)))
    expected = [bindings['python']['path'], str(Path(__file__).resolve()), '--config', str(config_path),
                '--config-sha256', args.config_sha256, '--phase', args.phase]
    guard = LOCAL / (run + '-observer/guard.json')
    require(sys.orig_argv == expected and live['status'] == 'running' and live['run_id'] == run_id
            and live['source_root'] == str(ROOT) and live['source_before'] == bindings['runner_sources']['candidate']
            and Path(receipt_path).resolve() == LOCAL / (run + '-receipt.json')
            and live['argv'] == [bindings['python']['path'], bindings['guard']['path'],
                                 '--evidence-dir', str(guard.parent), '--', *expected], 'Actual runner/observer/launcher argv differs')
    LOCAL.mkdir(parents=True, exist_ok=True)
    with intent.open('x') as handle:
        json.dump({'schema': 1, 'experiment': NAME, 'phase': args.phase, 'status': 'launch-intent-not-completion',
                   'config': {'path': str(config_path), 'sha256': args.config_sha256}, 'argv': sys.orig_argv,
                   'command': command, 'run_id': run_id, 'run_receipt': str(Path(receipt_path).resolve()),
                   'created_unix_ns': time.time_ns(), 'admission': False}, handle, indent=2)
        handle.write('\n')
    os.environ['PAGURA_F32_LAUNCH_INTENT'] = str(intent)
    os.environ['PAGURA_F32_LAUNCH_SHA256'] = descriptor(intent)['sha256']
    os.execv(bindings['python']['path'], command)


if __name__ == '__main__':
    main()
