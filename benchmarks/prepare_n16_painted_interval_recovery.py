#!/usr/bin/env python3
"""Freeze new recovery files; never launch a benchmark or read partial timings."""
import argparse
import copy
import json
from pathlib import Path
import sys

sys.dont_write_bytecode = True
import n16_painted_interval_recovery as recovery
from evaluate_n16_output import load, require
from evaluate_n16_painted_interval_wasi import file_hash, pinned


def descriptor(path):
    path = Path(path).resolve(strict=True)
    return {'path': str(path), 'sha256': file_hash(path)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--decision-reason', required=True)
    args = parser.parse_args()
    require(args.decision_reason.strip(), 'explicit recovery reason required')
    require(Path.cwd().resolve() == recovery.ROOT, 'run from candidate root')
    config_path = recovery.LOCAL / 'confirmation-config.json'
    plan_path = recovery.RESULTS / (recovery.NAME + '-plan.json')
    bindings_path = recovery.RESULTS / (recovery.NAME + '-bindings.json')
    require(not any(path.exists() for path in (config_path, plan_path, bindings_path, recovery.LOCAL / recovery.RUN,
                                               recovery.LOCAL / (recovery.RUN + '-launch.json'))), 'recovery already frozen/launched')
    original = pinned(recovery.ORIGINAL_CONFIG, recovery.ANCHORS['config'])
    old_plan = recovery.pin(original['plan']); old_bindings = recovery.pin(original['bindings'])
    require(original['plan']['sha256'] == recovery.ANCHORS['plan']
            and original['bindings']['sha256'] == recovery.ANCHORS['bindings'], 'original plan/bindings changed')
    old_local = recovery.RESULTS / 'local/n16-painted-interval'
    excluded = {key: descriptor(old_local / path) for key, path in (
        ('receipt', 'confirmation-001-receipt.json'), ('guard', 'confirmation-001-observer/guard.json'),
        ('report', 'confirmation-001/report.json'), ('intent', 'confirmation-001-launch.json'))}
    require(excluded['receipt']['sha256'] == recovery.ANCHORS['receipt']
            and excluded['guard']['sha256'] == recovery.ANCHORS['guard'], 'interruption anchor changed')
    guard = recovery.pin(excluded['guard'])
    excluded['snapshots'] = []
    for event in guard['events']:
        name = event['snapshot']
        require(Path(name).name == name, 'unsafe snapshot name')
        excluded['snapshots'].append(descriptor(old_local / 'confirmation-001-observer' / name))
    declaration = {'decision_reason': args.decision_reason, 'excluded_confirmation': excluded,
                   'excluded_samples': 'all', 'partial_timing_inspected': False, 'automatic_retry': False,
                   'scope': 'Separately declared recovery after observed external contention; original launch allowance remains exhausted.',
                   'declaration': descriptor(recovery.ROOT / recovery.DOC)}
    plan = recovery.make_plan(old_plan, declaration)
    preflight = {key: descriptor(old_local / path) for key, path in (
        ('receipt', 'native-preflight-002-receipt.json'), ('guard', 'native-preflight-002-observer/guard.json'),
        ('artifact', 'native-preflight-002/report.json'))}
    evidence = {}
    def collect(value):
        if isinstance(value, dict):
            if 'path' in value and 'sha256' in value:
                require(file_hash(Path(value['path'])) == value['sha256'], 'existing evidence pin differs')
                evidence[value['path']] = value['sha256']
            for child in value.values():
                collect(child)
        elif isinstance(value, list):
            for child in value:
                collect(child)
    collect(original); collect(excluded); collect(preflight); collect(declaration['declaration'])
    collect({'path': str(recovery.ORIGINAL_CONFIG), 'sha256': recovery.ANCHORS['config']})
    for path, digest in old_bindings['bound_files_sha256'].items():
        require(file_hash(recovery.ROOT / path) == digest, 'original helper changed before freeze')
    bindings = copy.deepcopy(old_bindings)
    bindings.update(experiment=recovery.NAME, original_evidence_files_sha256=evidence,
                    original_config={'path': str(recovery.ORIGINAL_CONFIG), 'sha256': recovery.ANCHORS['config']})
    recovery.LOCAL.mkdir(parents=True, exist_ok=True)
    def write(path, value):
        with path.open('x') as stream:
            stream.write(json.dumps(value, indent=2, allow_nan=False) + '\n')
    write(plan_path, plan)
    for relative in (recovery.DRIVER, recovery.AUDITOR, recovery.FREEZER, recovery.DOC,
                     str(plan_path.relative_to(recovery.ROOT))):
        bindings['bound_files_sha256'][relative] = file_hash(recovery.ROOT / relative)
    write(bindings_path, bindings)
    config = {key: copy.deepcopy(original[key]) for key in ('python', 'python_executable', 'guard', 'prerequisites')}
    config.update(schema=1, experiment=recovery.NAME, status='frozen-before-launch', phase='confirmation',
                  original_config={'path': str(recovery.ORIGINAL_CONFIG), 'sha256': recovery.ANCHORS['config']},
                  identities=bindings['identities'], plan=descriptor(plan_path), bindings=descriptor(bindings_path),
                  native_preflight=preflight, recovery=declaration)
    write(config_path, config)
    digest = file_hash(config_path)
    recovery.validate(config_path, digest)
    driver, observer, measurement = recovery.commands(config, config_path, digest)
    print(json.dumps({'config': {'path': str(config_path), 'sha256': digest}, 'driver_argv': driver,
        'observer_argv': observer, 'measurement_arguments': measurement,
        'required_runner_artifacts': [str(config_path), str(recovery.LOCAL / (recovery.RUN + '-launch.json')),
            str(recovery.LOCAL / recovery.RUN / 'report.json'), str(recovery.LOCAL / (recovery.RUN + '-observer/guard.json'))],
        'audit_argv': [config['python']['path'], str(recovery.ROOT / recovery.AUDITOR),
                      '--config', str(config_path), '--config-sha256', digest], 'launched_nothing': True}, indent=2))


if __name__ == '__main__':
    main()
