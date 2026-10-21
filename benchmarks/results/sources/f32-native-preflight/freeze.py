"""Freeze completed equal-profile builds and one input core before any preflight."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys

sys.dont_write_bytecode = True
ROOT = Path.cwd().resolve()
sys.path[:0] = [str(ROOT / 'benchmarks'), str(ROOT / 'tools')]
import f32_painted_interval as experiment
from f32_interval_admission import descriptor, file_hash, require, success, validate_guard, verify_core
from paired_render import source_identity


def read(path):
    return json.loads(Path(path).read_bytes())


def write(path, value):
    with path.open('x') as output:
        json.dump(value, output, indent=2)
        output.write('\n')


def freeze(evidence):
    results = ROOT / 'benchmarks/results'
    local = results / 'local/f32-painted-interval'
    selection = read(local / 'build-selection-001.json')
    identities, native_builds, runner_sources = {}, {}, {}
    portable = {profile: {'schema': 1, 'target': 'wasm32-wasip1', 'profile': 'release',
        'target_feature': feature, 'rustflags': '-C target-feature=' + feature,
        'cargo_encoded_rustflags': None}
        for profile, feature in (('simd', '+simd128'), ('scalar', '-simd128'))}
    require(selection['schema'] == 1 and selection['experiment'] == 'f32-painted-interval'
            and len(selection['builds']) == 6
            and {(item['side'], item['kind']) for item in selection['builds']}
            == {(side, kind) for side in ('baseline', 'candidate') for kind in ('native', 'simd', 'scalar')},
            'Expected exactly one build for every side/kind')
    target_dirs = set()
    for item in sorted(selection['builds'], key=lambda item: item['kind'] != 'native'):
        side, kind = item['side'], item['kind']
        receipt, context, guard = read(item['receipt']), read(item['context']), read(item['guard'])
        success(receipt)
        validate_guard(guard, 'strict')
        require(receipt['source_root'] == item['root'] == receipt['cwd'] == context['root'], 'Wrong build checkout')
        for key in ('context', 'guard', 'binary', 'log'):
            require(receipt['artifacts'].get(item[key]) == file_hash(Path(item[key])), 'Build artifact not bound: ' + key)
        require(context['CARGO_TARGET_DIR'] == item['target'] and item['target'] not in target_dirs,
                'Build targets are not distinct')
        target_dirs.add(item['target'])
        identity = source_identity(item['root'])
        require(receipt['source_before']['commit'] == identity['commit'], 'Build HEAD differs')
        current_runner = subprocess.check_output([sys.orig_argv[0], '-c',
            'from benchkit.config import source_digest; print(source_digest())'],
            cwd=Path(item['root']) / 'tools', text=True).strip()
        require(receipt['source_before']['sha256'] == current_runner, 'Build runner source differs')
        example = Path(item['root']) / 'crates/pagura-render/examples/scene_index_profile.rs'
        require(example.read_bytes() == (ROOT / 'crates/pagura-render/examples/scene_index_profile.rs').read_bytes(),
                'Profile example differs')
        messages = [json.loads(line) for line in Path(item['log']).read_text().splitlines() if line.startswith('{')]
        finished = [m for m in messages if m.get('reason') == 'build-finished']
        artifacts = [m for m in messages if m.get('reason') == 'compiler-artifact'
            and m.get('target', {}).get('name') == 'scene_index_profile' and m.get('executable')]
        require(len(finished) == len(artifacts) == 1 and finished[0]['success'] is True
                and artifacts[0]['executable'] == item['binary'], 'Actual Cargo artifact differs')
        if kind == 'native':
            identities[side] = {**identity, 'binary': item['binary'], 'binary_sha256': file_hash(Path(item['binary']))}
            runner_sources[side] = receipt['source_before']
            native_builds[side] = {'receipt': item['receipt'], 'receipt_sha256': file_hash(Path(item['receipt'])),
                'context': descriptor(item['context']), 'guard': descriptor(item['guard'])}
        else:
            require(identity == {k: identities[side][k] for k in identity}
                    and receipt['source_before'] == runner_sources[side], 'Native/WASI sources differ')
            portable[kind][side] = {**descriptor(item['binary']), 'source_root': item['root'],
                'source': receipt['source_before'], 'receipt': item['receipt'],
                'receipt_sha256': file_hash(Path(item['receipt'])), 'context': descriptor(item['context']),
                'guard': descriptor(item['guard'])}
    experiment.require_baseline(identities['baseline']['root'])
    template = results / 'f32-painted-interval-plan.template.json'
    plan_path = results / 'f32-painted-interval-plan.json'
    draft = plan_path.read_bytes()
    require(draft == template.read_bytes(), 'Only the unchanged unexecuted template may be frozen')
    archive = local / 'plan-draft-before-freeze.json'
    binding_path = results / 'f32-painted-interval-preflight-bindings.json'
    portable_paths = {kind: results / ('f32-painted-interval-wasi-' + kind + '-build-bindings.json')
                      for kind in portable}
    require(all(not path.exists() and not path.is_symlink()
                for path in (archive, binding_path, *portable_paths.values())), 'Freeze destination already exists')
    plan = read(template)
    require(plan['status'] == 'draft-not-executable', 'Wrong template')
    plan.update(status='frozen-before-pilot', identities_to_freeze=identities,
        cells=[{'case': n, 'mode': m, 'operation': o,
                'role': 'target' if (n, o) == ('sparse64', 'full') else 'protected'}
               for n, m, o in experiment.CELLS])
    # Stage bytes privately. Hard-link publication is exclusive, and its inode
    # identifies files owned by this attempt even if a signal interrupts link().
    draft_stage, plan_stage = evidence / 'draft-plan.json', evidence / 'frozen-plan.json'
    with draft_stage.open('xb') as output:
        output.write(draft)
    write(plan_stage, plan)
    files = set((ROOT / 'benchmarks').glob('*.py')) | set((ROOT / 'benchmarks').glob('*.mjs'))
    for directory in ('tools/benchkit', 'tools/fixturegen'):
        files.update((ROOT / directory).rglob('*.py'))
    for directory in (results / 'sources').glob('f32-*'):
        files.update(directory.rglob('*.py'))
        files.update(directory.rglob('*.mjs'))
    files.update([plan_path, ROOT / 'tools/runner.py', ROOT / 'tools/check.py',
        ROOT / '.cargo/config.toml', ROOT / 'rust-toolchain.toml',
        ROOT / 'crates/pagura-render/examples/scene_index_profile.rs',
        results / 'sources/analytic19-conversion/load_observer.py'])
    links = []
    def publish(stage, destination):
        links.append((stage, destination))
        os.link(stage, destination)  # Never replaces an existing destination.
    try:
        publish(draft_stage, archive)
        pending = evidence / 'plan-install.pending'
        os.link(plan_stage, pending)
        require(plan_path.read_bytes() == draft, 'Draft changed before publication')
        os.replace(pending, plan_path)
        bindings = {'schema': 1, 'experiment': 'f32-painted-interval', 'stage': 'preflight-inputs',
            'plan': descriptor(plan_path), 'identities': identities, 'native_builds': native_builds,
            'runner_sources': runner_sources, 'candidate_runner_source_sha256': runner_sources['candidate']['sha256'],
            'python': descriptor(sys.orig_argv[0]), 'python_executable': descriptor(sys.executable),
            'guard': descriptor(results / 'sources/analytic19-conversion/load_observer.py'),
            'bound_files_sha256': {str(p.relative_to(ROOT)): file_hash(p) for p in sorted(files)}}
        verify_core(bindings)
        staged = []
        for kind, data in portable.items():
            stage = evidence / (kind + '-build-bindings.json')
            write(stage, data)
            staged.append((stage, portable_paths[kind]))
        stage = evidence / 'preflight-bindings.json'
        write(stage, bindings)
        staged.append((stage, binding_path))  # Input core is the final publication.
        for source, destination in staged:
            publish(source, destination)
        result = {'status': 'completed', 'plan': descriptor(plan_path), 'bindings': descriptor(binding_path),
                  'portable': {kind: descriptor(path) for kind, path in portable_paths.items()},
                  'selection': descriptor(local / 'build-selection-001.json')}
        write(evidence / 'completed.json', result)
        return result
    except BaseException:
        errors = []
        for stage, destination in reversed(links):
            try:
                if destination.exists() and destination.samefile(stage):
                    destination.unlink()
            except BaseException as error:
                errors.append(f'{destination}: {type(error).__name__}: {error}')
        try:
            if plan_path.exists() and plan_path.samefile(plan_stage):
                pending = evidence / 'plan-restore.pending'
                os.link(draft_stage, pending)
                os.replace(pending, plan_path)
            require(plan_path.read_bytes() == draft, 'Canonical plan not restored; inspect retained evidence')
        except BaseException as error:
            errors.append(f'{plan_path}: {type(error).__name__}: {error}')
        write(evidence / 'rollback.json', {'restored': not errors, 'errors': errors})
        if errors:
            raise RuntimeError('Freeze rollback incomplete; inspect ' + str(evidence))
        raise


def main():
    evidence = ROOT / 'benchmarks/results/local/f32-painted-interval/freeze-001'
    evidence.mkdir(parents=False, exist_ok=False)
    try:
        result = freeze(evidence)
    except BaseException as error:
        write(evidence / 'failed.json', {'status': 'failed', 'type': type(error).__name__, 'message': str(error)})
        raise
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    previous = {number: signal.getsignal(number) for number in (signal.SIGINT, signal.SIGTERM)}
    def cancel(number, _frame):
        raise KeyboardInterrupt(f'Freeze cancelled by signal {number}')
    try:
        for number in previous:
            signal.signal(number, cancel)
        main()
    finally:
        for number, handler in previous.items():
            signal.signal(number, handler)
