"""Build frozen qualification probes, never run PDF workloads. Standard library only."""
from pathlib import Path
import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import struct
import subprocess
import sys
import tarfile
import time
import tomllib

HERE = Path(__file__).resolve().parent
RUST = '1.99.1'
RUST_COMMIT = '58cf8f9c35b355199727627d07ae34a4e2846d67'
FEATURES = ['bundled-data', 'cjk-fonts']
def plans(preset):
    return [('api', 'native', preset['api']['rust'], preset['api']['package']),
            *[('runtime', variant, preset['runtime']['rust'], preset['runtime']['package'])
              for variant in ['native', 'simd', 'scalar']],
            ('memory', 'native', preset['memory_native']['rust'], preset['memory_native']['package']),
            *[('memory', variant, preset['memory_wasi']['rust'], preset['memory_wasi']['package'])
              for variant in ['simd', 'scalar']]]


ENV_KEYS = ['RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER',
            'RUSTC_WORKSPACE_WRAPPER', 'RUSTC', 'CARGO_BUILD_TARGET',
            'CARGO_INCREMENTAL', 'CARGO_BUILD_JOBS', 'MACOSX_DEPLOYMENT_TARGET',
            'SDKROOT', 'CARGO_HOME', 'RUSTUP_TOOLCHAIN', 'PAGURA_QUAL_SOURCE_REVISION']
GITHUB_KEYS = ['GITHUB_SERVER_URL', 'GITHUB_REPOSITORY', 'GITHUB_RUN_ID',
               'GITHUB_RUN_ATTEMPT', 'GITHUB_SHA', 'GITHUB_REF', 'GITHUB_WORKFLOW_REF',
               'GITHUB_WORKFLOW_SHA', 'GITHUB_JOB', 'RUNNER_OS', 'RUNNER_ARCH',
               'ImageOS', 'ImageVersion']


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def write(path, value):
    Path(path).write_text(json.dumps(value, indent=2) + '\n')


def output(command, cwd=None, env=None):
    return subprocess.check_output(command, cwd=cwd, env=env, text=True)


def validate_sha(value):
    if not re.fullmatch('[0-9a-f]{40}', value):
        raise ValueError('A full lowercase immutable source SHA is required')
    return value


def bundle(expected):
    path = HERE / 'bundle.json'
    if digest(path) != expected:
        raise ValueError('Reviewed probe bundle manifest differs')
    manifest = json.loads(path.read_text())
    for name, sha in manifest['files'].items():
        if Path(name).name != name or digest(HERE / name) != sha:
            raise ValueError('Probe/build script differs from reviewed source')
    return manifest


def fixed_paths():
    workspace = Path(os.environ['GITHUB_WORKSPACE']).resolve()
    temporary = Path(os.environ['RUNNER_TEMP']).resolve()
    work = temporary / 'pagura-qualified-probe-work'
    return workspace / 'source', work / 'project', work / 'target', work / 'cargo-home'


def clean_source(source, expected):
    if output(['git', 'rev-parse', 'HEAD'], source).strip() != expected:
        raise ValueError('Source checkout SHA differs')
    if output(['git', 'status', '--porcelain'], source).strip():
        raise ValueError('Source checkout is not clean')
    origin = output(['git', 'config', '--get', 'remote.origin.url'], source).strip()
    allowed = os.environ['GITHUB_SERVER_URL'] + '/' + os.environ['GITHUB_REPOSITORY']
    if origin.removesuffix('.git') != allowed:
        raise ValueError('Only the same repository checkout is allowed')


def source_inventory(source):
    records = []
    tree = subprocess.check_output(['git', 'ls-tree', '-rz', '--full-tree', 'HEAD'], cwd=source)
    for record in tree.split(b'\0'):
        if not record:
            continue
        info, name = record.split(b'\t', 1)
        mode, kind, git_blob = info.decode().split()
        relative = name.decode('utf-8')
        path = source / relative
        if kind != 'blob' or mode not in ['100644', '100755'] or path.is_symlink():
            raise ValueError('Unsupported symlink/submodule in source provenance')
        data = path.read_bytes()
        blob = hashlib.sha1(b'blob ' + str(len(data)).encode() + b'\0' + data).hexdigest()
        if blob != git_blob:
            raise ValueError('Actual source bytes differ from the recorded Git blob')
        records.append({'path': relative, 'git_blob': git_blob, 'git_mode': mode,
                        'sha256': hashlib.sha256(data).hexdigest(), 'bytes': len(data)})
    return records


def release_profile(source):
    text = (source / 'Cargo.toml').read_text()
    start = text.index('[profile.release]')
    stop = text.index('[profile.bench]', start)
    raw = text[start:stop]
    return raw, tomllib.loads(raw)['profile']['release']


def build_env(sha, cargo_home, variant):
    env = os.environ.copy()
    # Preserve source .cargo/config.toml; exclude ambient compiler overrides.
    for key in list(env):
        if (key in ENV_KEYS or key in ['CARGO_TARGET_DIR', 'RUSTC_BOOTSTRAP', 'RUSTDOCFLAGS',
                                      'CARGO_ENCODED_RUSTDOCFLAGS']
                or key.startswith('CARGO_PROFILE_') or key.startswith('CARGO_TARGET_')):
            env.pop(key, None)
    env.update(CARGO_HOME=str(cargo_home), CARGO_INCREMENTAL='0', CARGO_BUILD_JOBS='2',
               RUSTUP_TOOLCHAIN=RUST, PAGURA_QUAL_SOURCE_REVISION=sha)
    if variant == 'native':
        env['MACOSX_DEPLOYMENT_TARGET'] = '15.0'
    if variant != 'native':
        sign = '-' if variant == 'scalar' else '+'
        env['RUSTFLAGS'] = f'-C target-feature={sign}simd128 -C overflow-checks=on'
    return env


def metadata_check(project, source, env, destination):
    raw = output(['cargo', '+' + RUST, 'metadata', '--locked', '--offline',
                  '--format-version', '1', '--manifest-path', str(project / 'Cargo.toml')], source, env)
    destination.write_text(raw)
    data = json.loads(raw)
    for package in data['packages']:
        manifest = Path(package['manifest_path']).resolve()
        if package['source'] is not None or not (manifest.is_relative_to(source) or manifest.is_relative_to(project)):
            raise ValueError('Probe build contains an unreviewed external dependency')
        if any(set(target['kind']) & {'custom-build', 'proc-macro'} for target in package['targets']):
            raise ValueError('Executable compile-time extensions are outside this qualification protocol')
    return digest(destination)


def native_header(path):
    header = path.read_bytes()[:8]
    if len(header) != 8 or struct.unpack('<II', header) != (0xfeedfacf, 0x0100000c):
        raise ValueError('Native module is not an arm64 Mach-O binary')
    return {'format': 'Mach-O64', 'cpu': 'arm64'}


def artifact_path(root, name):
    path = root / name
    if Path(name).is_absolute() or '..' in Path(name).parts or path.is_symlink() or not path.resolve().is_relative_to(root.resolve()):
        raise ValueError('Unsafe recorded artifact path')
    return path


def toolchain_inventory(source):
    sysroot = Path(output(['rustc', '+' + RUST, '--print', 'sysroot'], source).strip())
    paths = [sysroot / 'bin/rustc', sysroot / 'bin/cargo']
    paths += list((sysroot / 'lib').glob('*.dylib'))
    for triple in ['aarch64-apple-darwin', 'wasm32-wasip1']:
        for directory in ['lib', 'codegen-backends']:
            paths += [p for p in (sysroot / 'lib/rustlib' / triple / directory).glob('*') if p.is_file()]
    return [{'path': str(p.relative_to(sysroot)), 'sha256': digest(p), 'bytes': p.stat().st_size}
            for p in sorted(set(paths))]


def verify_role(root):
    r = json.loads((root / 'receipt.json').read_text())
    if not r['complete'] or len(r['builds']) != 7:
        raise ValueError('Incomplete seven-module source role')
    m = bundle(r['probe_bundle_sha256'])
    if r['builder_sha256'] != digest(__file__):
        raise ValueError('Builder changed after producing the receipt')
    preset = json.loads((HERE / 'presets.json').read_text())[r['preset']]
    if r['preset_definition'] != preset:
        raise ValueError('Recorded protocol preset differs')
    provenance = root / 'provenance'
    if digest(provenance / 'source-files.json') != r['source_files_sha256']:
        raise ValueError('Source inventory changed')
    if digest(provenance / 'source-archive.tar.gz') != r['source_archive_sha256']:
        raise ValueError('Frozen source archive changed')
    inventory = {e['path']: e for e in json.loads((provenance / 'source-files.json').read_text())}
    seen = set()
    with tarfile.open(provenance / 'source-archive.tar.gz', 'r:gz') as tar:
        for member in tar:
            if not member.isfile() or member.name not in inventory or member.name in seen:
                raise ValueError('Unexpected source archive member')
            e = inventory[member.name]
            if member.size != e['bytes']:
                raise ValueError('Source archive size differs')
            stream = tar.extractfile(member)
            if stream is None:
                raise ValueError('Missing source archive bytes')
            data = stream.read()
            blob = hashlib.sha1(b'blob ' + str(len(data)).encode() + b'\0' + data).hexdigest()
            if hashlib.sha256(data).hexdigest() != e['sha256'] or blob != e['git_blob']:
                raise ValueError('Source archive bytes differ from their Git blob')
            seen.add(member.name)
    if seen != set(inventory):
        raise ValueError('Frozen source archive omitted tracked files')
    for name, sha in r['source_proofs'].items():
        if digest(provenance / 'source' / name) != sha or inventory[name]['sha256'] != sha:
            raise ValueError('Frozen root source/config proof changed')
    toolchain = provenance / 'toolchain-files.json'
    if digest(toolchain) != r['toolchain_files_sha256'] or json.loads(toolchain.read_text()) != toolchain_inventory(Path(r['source_path'])):
        raise ValueError('Compiler/backend/native/WASM stdlib bytes changed')
    profile = tomllib.loads((provenance / 'source/Cargo.toml').read_text())['profile']['release']
    if profile != r['profile'] or r['features'] != FEATURES:
        raise ValueError('Source profile/features differ')
    for build, expected in zip(r['builds'], plans(preset), strict=True):
        probe, variant, rust_name, package = expected
        if (build['probe'], build['variant'], build['package']) != (probe, variant, package):
            raise ValueError('Wrong probe, variant or original package name')
        module = artifact_path(root, build['module'])
        if digest(module) != build['module_sha256'] or module.stat().st_size != build['module_bytes'] or module.stat().st_mode != build['module_mode']:
            raise ValueError('Frozen module changed')
        directory = provenance / (probe + '-' + variant)
        for name, sha in build['proof_files'].items():
            if digest(artifact_path(root, name)) != sha:
                raise ValueError('Recorded build/probe/log/header proof changed')
        if digest(directory / 'main.rs') != m['files'][rust_name] or build['probe_source_sha256'] != m['files'][rust_name]:
            raise ValueError('Compiled probe differs from its exact preset')
        for name, field in [('Cargo.toml', 'external_manifest_sha256'), ('Cargo.lock', 'external_lock_sha256'), ('cargo-metadata.json', 'dependency_graph_sha256')]:
            if digest(directory / name) != build[field]:
                raise ValueError('Recorded Cargo input/graph changed')
        external = tomllib.loads((directory / 'Cargo.toml').read_text())
        if external['package']['name'] != package or external['profile']['release'] != profile:
            raise ValueError('External package/profile differs')
        dep = external['dependencies']['pagura']
        if dep['path'] != str(Path(r['source_path']) / 'crates/pagura') or dep['features'] != FEATURES:
            raise ValueError('Source dependency/features differ')
        for command in build['commands']:
            if command['exit'] != 0 or digest(artifact_path(root, command['log'])) != command['log_sha256']:
                raise ValueError('Recorded command/log is not successful and unchanged')
        expected_env = build_env(r['source_sha'], Path(r['fixed_project_path']).parent / 'cargo-home', variant)
        if build['compile_env'] != {key: expected_env.get(key) for key in ENV_KEYS}:
            raise ValueError('Recorded variant compile options differ')
        if variant == 'native':
            native_header(module)
            deployment = (directory / 'deployment.txt').read_text()
            minimum = re.findall(r'^\s*minos\s+([0-9]+(?:\.[0-9]+)*)\s*$', deployment, re.M)
            parts = list(map(int, minimum[0].split('.'))) if len(minimum) == 1 else []
            if len(parts) > 3 or tuple(parts + [0] * (3 - len(parts))) != (15, 0, 0):
                raise ValueError('Native Mach-O deployment minimum differs from paired 15.0 protocol')
        elif module.read_bytes()[:8] != b'\0asm\1\0\0\0':
            raise ValueError('WASM module changed format')
    return r


def build_role(args):
    expected = validate_sha(args.sha)
    manifest = bundle(args.bundle_sha256)
    preset = json.loads((HERE / 'presets.json').read_text())[args.preset]
    source, project, target, cargo_home = fixed_paths()
    if platform.system() != 'Darwin' or platform.machine() != 'arm64':
        raise ValueError('An existing macOS ARM64 GitHub-hosted runner is required')
    if os.environ.get('GITHUB_ACTIONS') != 'true':
        raise ValueError('This builder is CI-only; local builds are not supported')
    for path in [project, target, cargo_home]:
        if path.exists():
            raise ValueError('Fresh fixed project/target/cargo-home boundary required')
    clean_source(source, expected)
    if tomllib.loads((source / 'rust-toolchain.toml').read_text())['toolchain']['channel'] != RUST:
        raise ValueError('Source toolchain differs from the reviewed compiler')
    out = Path(args.output).resolve() / args.role
    out.mkdir(parents=True, exist_ok=False)
    (out / 'modules').mkdir()
    (out / 'provenance').mkdir()
    initial = source_inventory(source)
    write(out / 'provenance/source-files.json', initial)
    source_archive = out / 'provenance/source-archive.tar.gz'
    with tarfile.open(source_archive, 'w:gz', compresslevel=6) as tar:
        for entry in initial:
            tar.add(source / entry['path'], arcname=entry['path'], recursive=False)
    raw_profile, profile = release_profile(source)
    proof = {}
    for name in ['Cargo.toml', 'Cargo.lock', '.cargo/config.toml', 'rust-toolchain.toml']:
        p = out / 'provenance/source' / name
        p.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source / name, p)
        proof[name] = digest(p)
    compiler = output(['rustc', '+' + RUST, '-Vv'], source)
    cargo = output(['cargo', '+' + RUST, '-Vv'], source)
    if f'commit-hash: {RUST_COMMIT}\n' not in compiler or 'host: aarch64-apple-darwin\n' not in compiler:
        raise ValueError('Compiler commit/host differs')
    host = {'platform': platform.platform(), 'python': sys.version,
            'uname': output(['uname', '-a']), 'sw_vers': output(['sw_vers']),
            'xcode': output(['xcodebuild', '-version']),
            'sdk_path': output(['xcrun', '--sdk', 'macosx', '--show-sdk-path']),
            'sdk_version': output(['xcrun', '--sdk', 'macosx', '--show-sdk-version']),
            'clang': output(['xcrun', 'clang', '--version'])}
    toolchain = toolchain_inventory(source)
    write(out / 'provenance/toolchain-files.json', toolchain)
    r = {'schema': 1, 'preset': args.preset, 'preset_definition': preset, 'complete': False, 'role': args.role, 'source_sha': expected,
         'source_tree': output(['git', 'rev-parse', 'HEAD^{tree}'], source).strip(),
         'repository': os.environ['GITHUB_REPOSITORY'], 'source_path': str(source),
         'fixed_project_path': str(project), 'fixed_target_path': str(target),
         'source_files_sha256': digest(out / 'provenance/source-files.json'),
         'source_archive_sha256': digest(source_archive),
         'toolchain_files_sha256': digest(out / 'provenance/toolchain-files.json'),
         'source_proofs': proof, 'profile': profile, 'features': FEATURES,
         'rustc': compiler, 'cargo': cargo, 'host': host,
         'workflow': {key: os.environ.get(key) for key in GITHUB_KEYS},
         'probe_bundle_sha256': args.bundle_sha256, 'probe_manifest': manifest,
         'builder_sha256': digest(__file__), 'builds': [],
         'scope': 'Build provenance only; no PDF workload, accuracy, timing, RSS, Store, linear-page or admission claim.'}
    receipt = out / 'receipt.json'
    write(receipt, r)
    try:
        for probe, variant, rust_name, package in plans(preset):
            clean_source(source, expected)
            for path in [project, target, cargo_home]:
                if path.exists():
                    raise ValueError('A previous variant left a dependency/build cache')
            if shutil.disk_usage(out).free < 3 * 1024**3:
                raise ValueError('Preserve the existing 3GiB physical fresh-build guard')
            directory = out / 'provenance' / (probe + '-' + variant)
            directory.mkdir()
            print(json.dumps({'event': 'build-start', 'role': args.role, 'probe': probe, 'variant': variant}), flush=True)
            (project / 'src').mkdir(parents=True)
            cargo_home.mkdir()
            shutil.copy2(HERE / rust_name, project / 'src/main.rs')
            dependency = 'pagura = {path=' + json.dumps(str(source / 'crates/pagura')) + ', features=["bundled-data", "cjk-fonts"]}\n'
            project_manifest = ('[workspace]\n[package]\nname=' + json.dumps(package)
                                + '\nversion="0.1.0"\nedition="2024"\n[dependencies]\n'
                                + dependency + raw_profile)
            (project / 'Cargo.toml').write_text(project_manifest)
            env = build_env(expected, cargo_home, variant)
            commands = []
            def command(arguments, log_name):
                log = directory / log_name
                start = time.monotonic()
                with log.open('w') as stream:
                    p = subprocess.run(arguments, cwd=source, env=env, stdout=stream,
                                       stderr=subprocess.STDOUT, timeout=1200)
                commands.append({'command': arguments, 'exit': p.returncode,
                                 'seconds': time.monotonic() - start,
                                 'log': str(log.relative_to(out)), 'log_sha256': digest(log)})
                p.check_returncode()
            command(['cargo', '+' + RUST, 'generate-lockfile', '--offline', '--manifest-path',
                     str(project / 'Cargo.toml')], 'generate-lock.log')
            graph_sha = metadata_check(project, source, env, directory / 'cargo-metadata.json')
            arguments = ['cargo', '+' + RUST, 'build', '--release', '--locked', '--offline',
                         '--manifest-path', str(project / 'Cargo.toml'), '--target-dir', str(target),
                         '-j', '2', '--verbose']
            if variant != 'native':
                arguments += ['--target', 'wasm32-wasip1']
            command(arguments, 'build.log')
            original = target / ('release/' + package if variant == 'native' else 'wasm32-wasip1/release/' + package + '.wasm')
            destination = out / 'modules' / (probe + '-' + variant + ('' if variant == 'native' else '.wasm'))
            shutil.copy2(original, destination)
            if digest(original) != digest(destination):
                raise ValueError('Frozen output differs from the actual build')
            for name in ['Cargo.toml', 'Cargo.lock']:
                shutil.copy2(project / name, directory / name)
            shutil.copy2(project / 'src/main.rs', directory / 'main.rs')
            if digest(directory / 'main.rs') != manifest['files'][rust_name]:
                raise ValueError('Compiled probe bytes differ')
            header = native_header(destination) if variant == 'native' else {'format': 'WASM1'}
            if variant == 'native':
                for label, cmd in [('mach-header', ['otool', '-hv', str(destination)]),
                                   ('linked-libraries', ['otool', '-L', str(destination)]),
                                   ('deployment', ['xcrun', 'vtool', '-show-build', str(destination)])]:
                    (directory / (label + '.txt')).write_text(output(cmd))
            elif destination.read_bytes()[:8] != b'\0asm\1\0\0\0':
                raise ValueError('Invalid WASM module header')
            build = {'probe': probe, 'variant': variant, 'package': package,
                     'module': str(destination.relative_to(out)), 'module_sha256': digest(destination),
                     'module_bytes': destination.stat().st_size, 'module_mode': destination.stat().st_mode,
                     'header': header, 'probe_source_sha256': manifest['files'][rust_name],
                     'external_manifest_sha256': digest(directory / 'Cargo.toml'),
                     'external_lock_sha256': digest(directory / 'Cargo.lock'),
                     'dependency_graph_sha256': graph_sha, 'fresh_target_before_build': True,
                     'compile_env': {key: env.get(key) for key in ENV_KEYS}, 'commands': commands}
            build['proof_files'] = {str(p.relative_to(out)): digest(p) for p in directory.iterdir() if p.is_file()}
            r['builds'].append(build)
            write(receipt, r)
            clean_source(source, expected)
            for path in [target, project, cargo_home]:
                shutil.rmtree(path)
            build['owned_ci_caches_reclaimed'] = True
            write(receipt, r)
            print(json.dumps({'event': 'module-frozen', 'role': args.role, 'probe': probe, 'variant': variant, 'sha256': build['module_sha256']}), flush=True)
        if source_inventory(source) != initial:
            raise ValueError('Tracked source bytes changed during build')
        r['complete'] = True
        write(receipt, r)
        verify_role(out)
    except Exception as error:
        r['error'] = {'kind': type(error).__name__, 'message': str(error)}
        write(receipt, r)
        raise


def boundary(args):
    source, project, target, cargo_home = fixed_paths()
    root = Path(args.output)
    d = verify_role(root / 'baseline')
    clean_source(source, d['source_sha'])
    inventory = json.loads((root / 'baseline/provenance/source-files.json').read_text())
    if source_inventory(source) != inventory:
        raise ValueError('Baseline source changed before the source switch')
    if any(path.exists() for path in [project, target, cargo_home]):
        raise ValueError('Source switch would retain a build/dependency cache')
    seal = {'receipt_sha256': digest(root / 'baseline/receipt.json'),
            'files': {str(p.relative_to(root / 'baseline')): digest(p)
                      for p in (root / 'baseline').rglob('*') if p.is_file()}}
    write(root / 'baseline-boundary.json', seal)
    print('baseline-boundary-sha256=' + digest(root / 'baseline-boundary.json'), flush=True)


def finalize(args):
    root = Path(args.output).resolve()
    roles = [verify_role(root / name) for name in ['baseline', 'candidate']]
    seal = json.loads((root / 'baseline-boundary.json').read_text())
    if digest(root / 'baseline/receipt.json') != seal['receipt_sha256']:
        raise ValueError('Baseline receipt changed after the source switch')
    for name, sha in seal['files'].items():
        if digest(artifact_path(root / 'baseline', name)) != sha:
            raise ValueError('A sealed baseline file changed')
    source, project, target, cargo_home = fixed_paths()
    clean_source(source, roles[1]['source_sha'])
    if source_inventory(source) != json.loads((root / 'candidate/provenance/source-files.json').read_text()):
        raise ValueError('Candidate source differs at finalization')
    if any(p.exists() for p in [project, target, cargo_home]):
        raise ValueError('Final CI dependency/build caches remain')
    if not all(d['complete'] and len(d['builds']) == 7 for d in roles):
        raise ValueError('Both roles require seven complete frozen builds')
    for key in ['rustc', 'cargo', 'host', 'source_path', 'fixed_project_path', 'fixed_target_path',
                'profile', 'features', 'probe_bundle_sha256', 'builder_sha256', 'preset',
                'preset_definition', 'toolchain_files_sha256']:
        if roles[0][key] != roles[1][key]:
            raise ValueError('Paired CI build environments differ: ' + key)
    if roles[0]['source_proofs']['.cargo/config.toml'] != roles[1]['source_proofs']['.cargo/config.toml']:
        raise ValueError('A changed target configuration needs an explicitly different protocol')
    for before, after in zip(roles[0]['builds'], roles[1]['builds'], strict=True):
        for field in ['probe', 'variant', 'package', 'probe_source_sha256']:
            if before[field] != after[field]:
                raise ValueError('Mismatched selected protocol: ' + field)
        for build, role in [(before, roles[0]), (after, roles[1])]:
            if build['compile_env']['PAGURA_QUAL_SOURCE_REVISION'] != role['source_sha']:
                raise ValueError('Compiled source-revision environment differs')
        a = {k: v for k, v in before['compile_env'].items() if k != 'PAGURA_QUAL_SOURCE_REVISION'}
        b = {k: v for k, v in after['compile_env'].items() if k != 'PAGURA_QUAL_SOURCE_REVISION'}
        if a != b:
            raise ValueError('Per-variant compile environments differ')
    inventory = []
    for path in sorted(root.rglob('*')):
        if path.is_file():
            if path.is_symlink():
                raise ValueError('Symlinks are not valid frozen artifacts')
            inventory.append({'path': str(path.relative_to(root)), 'sha256': digest(path),
                              'bytes': path.stat().st_size, 'mode': path.stat().st_mode})
    manifest = root / 'manifest.json'
    write(manifest, {'schema': 1, 'complete': True, 'sources': {d['role']: d['source_sha'] for d in roles},
                     'files': inventory, 'workflow': roles[0]['workflow'],
                     'scope': 'Matched source-fresh CI builds only. Local oracle/parity/cost/memory gates remain mandatory.'})
    archive = root.parent / 'qualification-probes.tar.gz'
    if archive.exists():
        raise ValueError('Refusing to replace a frozen transport archive')
    with tarfile.open(archive, 'w:gz', compresslevel=6) as tar:
        for path in sorted(root.rglob('*')):
            if path.is_file():
                tar.add(path, arcname='qualification-probes/' + str(path.relative_to(root)), recursive=False)
    transport = {'schema': 1, 'complete': True, 'archive_sha256': digest(archive),
                 'archive_bytes': archive.stat().st_size, 'manifest_sha256': digest(manifest),
                 'uncompressed_file_bytes': sum(r['bytes'] for r in inventory) + manifest.stat().st_size,
                 'files': len(inventory) + 1, 'sources': {d['role']: d['source_sha'] for d in roles}}
    write(root.parent / 'qualification-probes-transport.json', transport)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    validate = commands.add_parser('validate')
    validate.add_argument('--bundle-sha256', required=True)
    validate.add_argument('--baseline', default='')
    validate.add_argument('--candidate', default='')
    validate.add_argument('--preset', default='')
    validate.add_argument('--github-output', required=True)
    build = commands.add_parser('build')
    build.add_argument('--role', choices=['baseline', 'candidate'], required=True)
    build.add_argument('--sha', required=True)
    build.add_argument('--preset', required=True)
    build.add_argument('--bundle-sha256', required=True)
    build.add_argument('--output', required=True)
    commands.add_parser('boundary').add_argument('--output', required=True)
    commands.add_parser('finalize').add_argument('--output', required=True)
    args = parser.parse_args()
    if args.command == 'validate':
        bundle(args.bundle_sha256)
        pair = json.loads((HERE / 'pair.json').read_text())
        baseline = validate_sha(args.baseline or pair['baseline'])
        candidate = validate_sha(args.candidate or pair['candidate'])
        preset = args.preset or pair['preset']
        if preset not in json.loads((HERE / 'presets.json').read_text()):
            raise ValueError('An exact named reviewed probe preset is required')
        if baseline == candidate:
            raise ValueError('Two distinct source commits are required')
        with Path(args.github_output).open('a') as stream:
            stream.write('baseline=' + baseline + '\ncandidate=' + candidate + '\npreset=' + preset + '\n')
    elif args.command == 'build':
        build_role(args)
    elif args.command == 'boundary':
        boundary(args)
    else:
        finalize(args)


if __name__ == '__main__':
    main()
