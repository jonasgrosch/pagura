"""Fixed F32 experiment admission/provenance checks; no timing or decision policy."""
from pathlib import Path
import sys

import f32_painted_interval as experiment
from paired_render import ROOT, expected_image, source_identity
from evaluate_n16_output import THRESHOLDS, cell_map, declared_cells, validate_guard
from evaluate_n16_painted_interval_wasi import decode, file_hash, pinned, read, require, success
from launch_n16_painted_interval import pin, completed_evidence, prior_run

NAME = 'f32-painted-interval'
RESULTS = ROOT / 'benchmarks/results'
LOCAL = RESULTS / 'local' / NAME
SIDES = ('baseline', 'candidate')
GATES = ('repository-validation', 'independent-rgba', 'tile-reuse', 'errors-resources',
         'layout', 'route-reach', 'wasi-portability')
CORE = ('schema', 'experiment', 'plan', 'identities', 'native_builds', 'runner_sources',
        'candidate_runner_source_sha256', 'bound_files_sha256', 'python', 'python_executable', 'guard')
HARNESS = {'benchmarks/' + name for name in (
    'f32_painted_interval.py', 'f32_painted_interval_fixtures.py',
    'n16_painted_interval.py', 'n16_painted_interval_fixtures.py', 'n16_output_u16.py',
    'n16_output_guarded.py', 'n16_output.py', 'opaque_composition.py', 'paired_render.py',
    'glyph_memo.py', 'glyph_memo_fixtures.py', 'scene_index.py', 'clip_viewport_fixtures.py')}
REQUIRED = HARNESS | {'benchmarks/' + name for name in (
    'f32_interval_admission.py', 'launch_f32_painted_interval.py', 'evaluate_f32_painted_interval.py',
    'evaluate_n16_output.py', 'evaluate_n16_painted_interval_wasi.py', 'launch_n16_painted_interval.py')}
REQUIRED |= {'crates/pagura-render/examples/scene_index_profile.rs',
             'benchmarks/results/' + NAME + '-plan.json',
             'benchmarks/results/sources/f32-native-preflight/run.py',
             'benchmarks/results/sources/f32-native-preflight/admit.py',
             'benchmarks/results/sources/f32-native-preflight/capture_native_build.py'}


def descriptor(path):
    return {'path': str(Path(path).resolve()), 'sha256': file_hash(Path(path).resolve())}


def runtime(bindings):
    for key, actual in (('python', sys.orig_argv[0]), ('python_executable', sys.executable)):
        item = bindings[key]
        require(item['path'] == str(Path(actual).resolve()) and file_hash(Path(item['path'])) == item['sha256'],
                'Use the pre-run pinned Python launcher and reported executable')


def verify_core(bindings):
    require(bindings['schema'] == 1 and bindings['experiment'] == NAME, 'Wrong binding schema/experiment')
    plan = pin(bindings['plan'])
    require(bindings['plan']['path'] == str(RESULTS / (NAME + '-plan.json'))
            and plan == experiment.frozen_plan(), 'Plan path/content differs')
    require((plan['case_count'], plan['cell_count'], plan['target_count'], plan['protected_count']) == (20, 56, 2, 54)
            and cell_map(plan['cells'], 56) == declared_cells(experiment, 20, 56, 'sparse64', 'full')
            and plan['thresholds'] == THRESHOLDS and plan['phases'] == experiment.PHASE_PLAN
            and plan['expected_verified_outputs'] == {'native_preflight': 112, 'pilot': 1120, 'confirmation': 22960}
            and plan['maximum_launched_pilots'] == plan['maximum_launched_confirmations'] == 1
            and plan['analysis'] == {'method': 'paired-percentile-bootstrap', 'resamples': 5000,
                                    'confidence': .95, 'seed': 42017}, 'Fixed matrix/statistics changed')
    require(bindings['candidate_runner_source_sha256'] == bindings['runner_sources']['candidate']['sha256'],
            'Runner source digest differs')
    required = REQUIRED | {Path(bindings['guard']['path']).relative_to(ROOT).as_posix()}
    require(required <= bindings['bound_files_sha256'].keys(), 'Required helper is not frozen')
    for relative, digest in bindings['bound_files_sha256'].items():
        path = (ROOT / relative).resolve()
        require(path.is_relative_to(ROOT) and file_hash(path) == digest, 'Frozen helper changed: ' + relative)
    require(bindings['bound_files_sha256'][Path(bindings['guard']['path']).relative_to(ROOT).as_posix()]
            == bindings['guard']['sha256'], 'Observer pin differs')
    runtime(bindings)
    experiment.require_baseline(bindings['identities']['baseline']['root'])
    commands, environments, configurations = [], [], []
    for side in SIDES:
        identity = bindings['identities'][side]
        require(identity == {key: plan['identities_to_freeze'][side][key] for key in identity}
                and source_identity(identity['root']) == {key: identity[key] for key in ('root', 'commit', 'source_sha256')}
                and file_hash(Path(identity['binary'])) == identity['binary_sha256'], 'Source/binary differs: ' + side)
        build = bindings['native_builds'][side]
        receipt = pinned(Path(build['receipt']), build['receipt_sha256'])
        success(receipt)
        context, guard = pin(build['context']), pin(build['guard'])
        validate_guard(guard, 'strict')
        require(receipt['source_root'] == identity['root']
                and receipt['source_before'] == bindings['runner_sources'][side]
                and receipt['source_before']['commit'] == identity['commit']
                and receipt['artifacts'].get(identity['binary']) == identity['binary_sha256']
                and all(receipt['artifacts'].get(build[k]['path']) == build[k]['sha256'] for k in ('context', 'guard')),
                'Native build/source/guard/artifact mismatch: ' + side)
        command = context['command']
        require(command and receipt['argv'][-len(command):] == command
                and context['root'] == identity['root'] and context['CARGO_BUILD_JOBS'] == '1', 'Build argv/context differs')
        capture = ROOT / 'benchmarks/results/sources/f32-native-preflight/capture_native_build.py'
        require(len(command) == 14 and Path(command[0]).is_absolute()
                and command[1:10] == ['build', '--locked', '--offline', '--release', '-p', 'pagura-render',
                                     '--example', 'scene_index_profile', '--target-dir']
                and command[11:] == ['-j', '1', '--message-format=json-render-diagnostics']
                and context['CARGO_TARGET_DIR'] == command[10] == str(Path(command[10]).resolve())
                and Path(command[10]).is_absolute()
                and identity['binary'] == str(Path(command[10]) / 'release/examples/scene_index_profile')
                and context['capture_sha256'] == bindings['bound_files_sha256'][capture.relative_to(ROOT).as_posix()]
                and context['capture_argv'] == [bindings['python']['path'], str(capture), build['context']['path'], '--', *command]
                and receipt['argv'][-len(context['capture_argv']):] == context['capture_argv']
                and guard['command'] == context['capture_argv']
                and context['cargo']['path'] == command[0]
                and file_hash(Path(command[0]).resolve()) == context['cargo']['sha256']
                and context['cargo_config']['path'] == str(Path(identity['root']) / '.cargo/config.toml')
                and file_hash(Path(context['cargo_config']['path'])) == context['cargo_config']['sha256'],
                'Native fixed capture/build command or tool/config pin differs')
        require(all(context.get(k) in (None, '') for k in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTUP_TOOLCHAIN',
                    'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'CARGO_BUILD_RUSTFLAGS')),
                'Native build has an ambient Rust override')
        # Only the explicit target directory may differ between the equal profiles.
        normalized = list(command)
        if '--target-dir' in normalized:
            normalized[normalized.index('--target-dir') + 1] = '<arm-target>'
        commands.append(normalized)
        environments.append({k: context[k] for k in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_JOBS')})
        configurations.append(context['cargo_config']['sha256'])
    require(commands[0] == commands[1] and environments[0] == environments[1]
            and configurations[0] == configurations[1], 'Native build profiles/configurations differ')
    return plan


def evidence(item, bindings, child=None):
    receipt, report = completed_evidence(item)
    guard = pin(item['guard'])
    validate_guard(guard, 'strict')
    require(receipt['source_root'] == str(ROOT) and receipt['source_before'] == bindings['runner_sources']['candidate']
            and receipt['artifacts'].get(item['guard']['path']) == item['guard']['sha256'], 'Prerequisite source/guard differs')
    if child is not None:
        require(guard['command'] == child and receipt['argv'] == [bindings['python']['path'], bindings['guard']['path'],
                '--evidence-dir', str(Path(item['guard']['path']).parent), '--', *child], 'Strict observer command differs')
    return receipt, report, guard


def verify_preflight(item, inputs, input_pin):
    directory = Path(item['artifact']['path']).parent
    command = [inputs['python']['path'], str(ROOT / 'benchmarks/results/sources/f32-native-preflight/run.py'),
               '--root', str(ROOT), '--bindings', input_pin['path'], '--bindings-sha256', input_pin['sha256'],
               '--output', str(directory)]
    receipt, report, _ = evidence(item, inputs, command)
    require(report['status'] == 'verified-native-preflight' and report['validation'] == 'native-preflight'
            and report['experiment'] == NAME and report['all_correct'] is True and report['identities_unchanged'] is True
            and report['expected_outputs'] == report['completed_outputs'] == len(report['records']) == 112
            and report['bindings_sha256'] == input_pin['sha256'] and report['identities'] == inputs['identities']
            and report['argv'] == command and report['run_id'] == receipt['run_id']
            and report['run_receipt'] == item['receipt']['path'], 'Native preflight is not complete for this freeze')
    artifacts = {a['path']: a for a in report['artifacts']}
    require(len(artifacts) == len(report['artifacts']), 'Duplicate preflight artifact')
    for path, entry in artifacts.items():
        raw = read(Path(path))
        require(Path(path).is_relative_to(directory) and len(raw) == entry['bytes']
                and file_hash(Path(path)) == entry['sha256'], 'Retained preflight artifact differs')
    for path, digest in report['pins'].items():
        require(file_hash(Path(path)) == digest, 'Preflight input/helper changed')
    require(all(report['pins'].get(inputs[k]['path']) == inputs[k]['sha256'] for k in ('python', 'python_executable')),
            'Preflight runtime was not pre-pinned')
    for name in experiment.CASES:
        pdf, oracle = experiment.fixture(name)
        for key, raw in (('pdf', pdf), ('oracle', oracle)):
            item = report['inputs'][name][key]
            require(item == artifacts.get(item['path']) and read(Path(item['path'])) == raw, 'Independent fixture differs')
    for index, (name, mode, operation) in enumerate(experiment.CELLS):
        width, height, rgb = expected_image(experiment.fixture(name)[1], operation)
        previous = None
        for side in SIDES:
            row = report['records'][index * 2 + SIDES.index(side)]
            output = row['output']
            require((row['cell'], row['case'], row['mode'], row['operation'], row['side']) == (index, name, mode, operation, side)
                    and row['verified'] is True and not row.get('error') and output == artifacts.get(output['path'])
                    and row['log'] == artifacts.get(row['log']['path']), 'Missing/reordered preflight record')
            require(row['command'] == [inputs['identities'][side]['binary'], report['inputs'][name]['pdf']['path'], mode,
                    operation, str(directory / f'pending-{index:02}-{side}.rgba'), '72'], 'Preflight renderer argv differs')
            process = row['process']
            require(type(process.get('exit_code')) is int and process['exit_code'] == 0 and process.get('status') != 'timeout',
                    'Preflight renderer failed')
            raw = read(Path(output['path']))
            header = f'RGBA8 {width} {height} premultiplied\n'.encode()
            pixels = raw[len(header):]
            require(raw.startswith(header) and len(pixels) == width * height * 4
                    and pixels[3::4] == b'\xff' * (width * height)
                    and all(pixels[c::4] == rgb[c::3] for c in range(3)), 'Strict independent RGBA differs')
            log = read(Path(row['log']['path'])).decode('utf-8')
            stages = [decode(line.encode()) for line in log.splitlines() if line.startswith('{')]
            require(len(stages) == 1 and all(stages[0].get(k) == v for k, v in
                    {'schema': 1, 'accuracy': mode, 'operation': operation, 'dpi': 72, 'width': width, 'height': height}.items()),
                    'Preflight stage identity differs')
            require(previous is None or previous == raw, 'Preflight same-mode bytes differ')
            previous = raw


def verify_admission(bindings):
    verify_core(bindings)
    require(bindings['stage'] == 'native-admission', 'Final native admission binding required')
    inputs = pin(bindings['preflight_bindings'])
    require(inputs['stage'] == 'preflight-inputs' and all(inputs[k] == bindings[k] for k in CORE), 'Frozen input core changed')
    verify_preflight(bindings['native_preflight'], inputs, bindings['preflight_bindings'])
    quality_receipt, quality, _ = evidence(bindings['quality'], bindings)
    require(quality['schema'] == 1 and quality['status'] == 'passed' and quality['experiment'] == NAME
            and quality['source'] == bindings['runner_sources']['candidate']
            and quality['gates'] == {name: 'passed' for name in GATES}, 'Quality gates incomplete')
    layouts = quality['layouts']
    types = ('Buffer', 'Option<Buffer>', 'Surface', 'MaskSink', 'Context', 'Vec<[u16;4]>', 'Range<usize>', '[u16;4]')
    require(len(layouts) == 24 and {(r['platform'], r['type']) for r in layouts} == {
        (t, n) for t in ('native', 'wasi-simd', 'wasi-scalar') for n in types}, 'Missing six-arm layouts')
    require(all(type(r[side][f]) is int and 0 < r['candidate'][f] <= r['baseline'][f]
                and type(r['delta'][f]) is int and r['delta'][f] == r['candidate'][f] - r['baseline'][f]
                for r in layouts for f in ('size', 'align') for side in SIDES), 'Layout grew or is unavailable')
    for name in ('repository_validation', 'six_arm_diagnostics', 'route_reach'):
        item = quality['evidence'][name]
        receipt = pin(item['receipt'])
        success(receipt)
        validate_guard(pin(item['guard']), 'strict')
        require(file_hash(Path(item['artifact']['path'])) == item['artifact']['sha256']
                and all(receipt['artifacts'].get(item[k]['path']) == item[k]['sha256'] for k in ('artifact', 'guard')),
                'Quality raw artifact/guard unbound')
    require(isinstance(quality['restorations'], list) and quality['restorations'], 'Missing exact restoration records')
    for item in quality['restorations']:
        require(file_hash(Path(item['path'])) == item['sha256']
                and quality_receipt['artifacts'].get(item['path']) == item['sha256'], 'Restoration record not bound by clean quality receipt')
    for profile, count in (('simd', 336), ('scalar', 24)):
        item = bindings['wasi'][profile]
        _, report, _ = evidence(item, bindings)
        audit = pin(item['audit'])
        require(report['status'] == 'completed' and audit['status'] == 'verified' and audit['runtime_profile'] == profile
                and audit['verified_outputs'] == count and audit['native_bindings_sha256'] == bindings['preflight_bindings']['sha256']
                and audit['frozen_plan_sha256'] == bindings['plan']['sha256']
                and audit['background_contention_observed'] is False
                and all(audit[k + '_sha256'] == item[v]['sha256'] for k, v in
                        (('receipt', 'receipt'), ('report', 'artifact'), ('guard', 'guard'))), 'WASI audit binding incomplete')
    require(bindings['wasi_review']['status'] == 'reviewed-for-native-experiment'
            and isinstance(bindings['wasi_review']['reason'], str) and bindings['wasi_review']['reason'].strip(),
            'Descriptive WASI evidence requires explicit coordinator review; it is not automatic admission')


def driver_command(bindings, phase):
    command = [bindings['python']['path'], str(ROOT / 'benchmarks/f32_painted_interval.py'),
               '--phase', phase, '--output', str(LOCAL / (phase + '-001'))]
    for side in SIDES:
        command += ['--' + side, bindings['identities'][side]['binary'],
                    '--' + side + '-source', bindings['identities'][side]['root']]
    return command


def verify_config(path, digest, phase):
    config = pinned(path, digest)
    require(config['experiment'] == NAME and config['phase'] == phase and phase in experiment.PHASE_PLAN
            and config['status'] == 'frozen-before-launch'
            and isinstance(config['decision_reason'], str) and config['decision_reason'].strip(), 'Invalid phase decision/config')
    require(config['bindings']['path'] == str(RESULTS / (NAME + '-bindings.json')), 'Wrong canonical admission binding')
    bindings = pin(config['bindings'])
    verify_admission(bindings)
    require(config['plan'] == bindings['plan'] and all(config[k] == bindings[k] for k in ('python', 'python_executable', 'guard')),
            'Phase changes frozen plan/runtime/observer')
    if phase == 'confirmation':
        receipt, _ = prior_run(config['prerequisites']['pilot'], 1120, config['bindings']['sha256'], config['plan']['sha256'])
        require(receipt['source_before'] == bindings['runner_sources']['candidate'], 'Pilot belongs to another source')
    return config, bindings
