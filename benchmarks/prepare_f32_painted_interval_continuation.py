"""Freeze one continuation amendment; no render/test/build or partial statistics."""
import argparse
import ast
import copy
import hashlib
import json
import os
from pathlib import Path
import sys

sys.dont_write_bytecode = True
import f32_painted_interval_continuation as continuation
from f32_interval_admission import pin, require
from benchkit.artifacts import read_bounded_regular


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--amendment', required=True, type=Path)
    parser.add_argument('--amendment-sha256', required=True)
    args = parser.parse_args()
    require(Path.cwd().resolve() == continuation.ROOT, 'Run from candidate checkout')
    amendment_pin = {'path': str(args.amendment.resolve()), 'sha256': args.amendment_sha256}
    amendment = pin(amendment_pin)
    old_config, old = continuation.original_evidence(amendment)
    # original_evidence hashes, but never decodes, the original partial report.
    plan_path = continuation.RESULTS / (continuation.NAME + '-plan.json')
    bindings_path = continuation.RESULTS / (continuation.NAME + '-bindings.json')
    config_path = continuation.LOCAL / 'confirmation-config.json'
    require(not any(p.exists() for p in (plan_path, bindings_path, config_path,
            continuation.LOCAL / continuation.RUN, continuation.LOCAL / (continuation.RUN + '-launch.json'),
            continuation.LOCAL / (continuation.RUN + '-driver-start.json'))), 'Continuation is already frozen or attempted')
    names = {continuation.DRIVER, continuation.FREEZER, continuation.DOC}
    require(not names & old['bound_files_sha256'].keys(), 'Continuation names collide with original helpers')
    hashes = {}
    for name in names:
        path = continuation.ROOT / name
        raw = read_bounded_regular(path, 8 << 20)
        if path.suffix == '.py':
            ast.parse(raw, filename=str(path))
        hashes[name] = hashlib.sha256(raw).hexdigest()
    def encode(value):
        return (json.dumps(value, indent=2, allow_nan=False) + '\n').encode()
    def future_pin(path, raw):
        return {'path': str(path), 'sha256': hashlib.sha256(raw).hexdigest()}
    plan_raw = encode(continuation.make_plan(pin(old['plan']), amendment_pin))
    plan_pin = future_pin(plan_path, plan_raw)
    bindings = copy.deepcopy(old)
    bindings.update(experiment=continuation.NAME, plan=plan_pin, amendment=amendment_pin,
                    original_config=amendment['original_config'])
    hashes[str(plan_path.relative_to(continuation.ROOT))] = plan_pin['sha256']
    bindings['bound_files_sha256'].update(hashes)
    bindings_raw = encode(bindings)
    config = {k: copy.deepcopy(old_config[k]) for k in ('python', 'python_executable', 'guard')}
    config.update(schema=1, experiment=continuation.NAME, status='frozen-before-launch',
                  amendment=amendment_pin, original_config=amendment['original_config'],
                  plan=plan_pin, bindings=future_pin(bindings_path, bindings_raw))
    config_raw = encode(config)
    config_pin = future_pin(config_path, config_raw)
    continuation.LOCAL.mkdir(parents=True, exist_ok=True)
    pending, published = {}, []
    try:
        for path, raw in ((plan_path, plan_raw), (bindings_path, bindings_raw), (config_path, config_raw)):
            temporary = path.with_name(path.name + '.pending')
            with temporary.open('xb') as output:
                output.write(raw)
            pending[path] = temporary
        for path, temporary in pending.items():
            os.link(temporary, path)  # Exclusive publication; never replace an existing frozen record.
            published.append(path)
        continuation.validate(config_path, config_pin['sha256'])
    except BaseException:
        for path in published:
            if path.exists() and path.stat().st_ino == pending[path].stat().st_ino:
                path.unlink()
        raise
    finally:
        for temporary in pending.values():
            temporary.unlink(missing_ok=True)
    launcher, driver, observer = continuation.commands(config, config_path, config_pin['sha256'])
    print(json.dumps({'config': config_pin, 'launcher_argv': launcher, 'post_exec_driver_argv': driver,
                     'observer_argv': observer, 'launched_nothing': True,
                     'required_final_runner_artifacts': [str(p) for p in (config_path,
                         continuation.LOCAL / (continuation.RUN + '-launch.json'),
                         continuation.LOCAL / (continuation.RUN + '-driver-start.json'),
                         continuation.LOCAL / continuation.RUN / 'report.json',
                         continuation.LOCAL / continuation.RUN / 'journal.jsonl',
                         continuation.LOCAL / (continuation.RUN + '-observer/guard.json'))]}, indent=2))


if __name__ == '__main__':
    main()
