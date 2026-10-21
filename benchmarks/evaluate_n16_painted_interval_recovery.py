#!/usr/bin/env python3
"""Authenticate the recovery launch, then reuse the unchanged 52-cell auditor."""
import argparse
import json
from pathlib import Path
import sys

sys.dont_write_bytecode = True
import n16_painted_interval_recovery as experiment
from evaluate_n16_output import audit, load, read, require, validate_guard
from evaluate_n16_painted_interval_wasi import file_hash, success


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', required=True, type=Path)
    parser.add_argument('--config-sha256', required=True)
    args = parser.parse_args()
    config, _, bindings = experiment.validate(args.config.resolve(), args.config_sha256)
    local, run = experiment.LOCAL, experiment.RUN
    receipt_path, guard_path = local / (run + '-receipt.json'), local / (run + '-observer/guard.json')
    intent_path, report_path = local / (run + '-launch.json'), local / run / 'report.json'
    receipt, guard, intent = load(receipt_path), load(guard_path), load(intent_path)
    success(receipt); validate_guard(guard, 'strict')
    driver, observer, measurement = experiment.commands(config, args.config.resolve(), args.config_sha256)
    require(receipt['argv'] == observer and guard['command'] == driver
            and intent['producer_argv'] == driver and intent['observer_argv'] == observer
            and intent['measurement_arguments'] == measurement
            and intent['execution'] == 'in-process call to unchanged paired_render.run_experiment', 'actual launch chain differs')
    require(intent['experiment'] == experiment.NAME and intent['phase'] == 'confirmation'
            and intent['status'] == 'launch-intent-not-completion'
            and intent['config'] == {'path': str(args.config.resolve()), 'sha256': args.config_sha256}
            and intent['run_id'] == receipt['run_id'] and intent['run_receipt'] == str(receipt_path)
            and intent['recovery'] == config['recovery'], 'intent binding differs')
    require(receipt['source_root'] == str(experiment.ROOT)
            and receipt['source_before'] == bindings['runner_sources']['candidate'], 'recovery source differs')
    require(guard['window_seconds'] == 5400 and guard['poll_seconds'] == 5, 'observer limits changed')
    for path in (args.config.resolve(), intent_path, report_path, guard_path):
        require(receipt['artifacts'].get(str(path)) == file_hash(path), 'receipt does not bind recovery artifact')
    snapshots = {}
    for event in guard['events']:
        name = event['snapshot']
        require(Path(name).name == name and read(guard_path.parent / name), 'missing/unsafe process snapshot')
        snapshots[str(guard_path.parent / name)] = file_hash(guard_path.parent / name)
    audit(experiment_module=experiment, experiment_name=experiment.NAME,
          stem=experiment.NAME, local_name=experiment.NAME, run=run,
          expected_harness_sources=experiment.HARNESS, audit_entrypoint=experiment.AUDITOR,
          expected_case_count=18, expected_cell_count=52, expected_guard_mode='strict')
    path = local / (run + '-analysis.json')
    summary = load(path)
    summary['recovery_provenance'] = {'config': intent['config'], 'intent_sha256': file_hash(intent_path),
        'excluded_confirmation': config['recovery']['excluded_confirmation'], 'all_original_samples_excluded': True,
        'snapshot_hashes_recorded_after_run': snapshots,
        'scope': 'Existing statistical/raw-output audit plus separately checked direct guard/combined-driver/intent chain; no admission decision.'}
    path.write_text(json.dumps(summary, indent=2) + '\n')


if __name__ == '__main__':
    main()
