#!/usr/bin/env python3
"""Authenticate the direct F32 launch chain, then reuse the fixed native audit."""
import argparse
from pathlib import Path
import sys

sys.dont_write_bytecode = True
import f32_painted_interval as experiment
from f32_interval_admission import (HARNESS, LOCAL, NAME, ROOT, decode, descriptor, driver_command,
                                    evidence, read, require, verify_config)
from evaluate_n16_output import audit


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('run', choices=('pilot-001', 'confirmation-001'))
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--config-sha256', required=True)
    args = parser.parse_args()
    phase = args.run.split('-')[0]
    config_path = args.config.resolve()
    config, bindings = verify_config(config_path, args.config_sha256, phase)
    intent_path, start_path = LOCAL / (args.run + '-launch.json'), LOCAL / (args.run + '-driver-start.json')
    intent, start = decode(read(intent_path)), decode(read(start_path))
    launcher = [bindings['python']['path'], str(ROOT / 'benchmarks/launch_f32_painted_interval.py'),
                '--config', str(config_path), '--config-sha256', args.config_sha256, '--phase', phase]
    item = {'receipt': descriptor(LOCAL / (args.run + '-receipt.json')),
            'artifact': descriptor(LOCAL / args.run / 'report.json'),
            'guard': descriptor(LOCAL / (args.run + '-observer/guard.json'))}
    receipt, report, _ = evidence(item, bindings, launcher)
    for path in (config_path, intent_path, start_path):
        require(receipt['artifacts'].get(str(path)) == descriptor(path)['sha256'], 'Final receipt misses launch evidence')
    require(intent['status'] == 'launch-intent-not-completion' and start['status'] == 'driver-start-not-completion'
            and intent['argv'] == launcher and start['argv'] == intent['command'] == driver_command(bindings, phase)
            and start['intent'] == descriptor(intent_path), 'Actual direct driver argv differs')
    for entry in (intent, start):
        require(entry['experiment'] == NAME and entry['phase'] == phase and entry['run_id'] == receipt['run_id']
                and entry['run_receipt'] == item['receipt']['path']
                and entry['config'] == {'path': str(config_path), 'sha256': args.config_sha256}, 'Launch run/config differs')
    require(report['environment']['run_id'] == receipt['run_id'], 'Report run differs')
    audit(experiment_module=experiment, experiment_name=NAME, stem=NAME, local_name=NAME, run=args.run,
          expected_case_count=20, expected_cell_count=56, target_case='sparse64', target_operation='full',
          expected_guard_mode='strict', expected_harness_sources=HARNESS,
          audit_entrypoint='benchmarks/evaluate_f32_painted_interval.py')


if __name__ == '__main__':
    main()
