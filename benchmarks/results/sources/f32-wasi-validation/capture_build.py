"""Capture one explicit WASI build context, then replace this process with it."""
import hashlib
import json
import os
from pathlib import Path
import sys

if len(sys.argv) < 4 or sys.argv[2] != '--':
    raise ValueError('capture_build.py NEW_CONTEXT_JSON -- /usr/bin/env -u CARGO_ENCODED_RUSTFLAGS RUSTFLAGS=... ABS_CARGO ...')
context_path, command = Path(sys.argv[1]).resolve(), sys.argv[3:]
if (len(command) != 20 or command[:3] != ['/usr/bin/env', '-u', 'CARGO_ENCODED_RUSTFLAGS']
        or command[3] not in ('RUSTFLAGS=-C target-feature=+simd128', 'RUSTFLAGS=-C target-feature=-simd128')
        or not Path(command[4]).is_absolute()
        or command[5:16] != ['build', '--locked', '--offline', '--release', '-p', 'pagura-render',
                            '--example', 'scene_index_profile', '--target', 'wasm32-wasip1', '--target-dir']
        or command[17:] != ['-j', '1', '--message-format=json-render-diagnostics']):
    raise ValueError('Command is not the fixed explicit-feature Cargo build')
target = Path(command[16])
if not target.is_absolute() or target != target.resolve():
    raise ValueError('Target directory must be a canonical absolute path')
allowed = {'CARGO_TARGET_DIR', 'CARGO_BUILD_JOBS', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS'}
unexpected = sorted(key for key, value in os.environ.items()
                    if value and key.startswith(('CARGO_', 'RUST')) and key not in allowed)
if unexpected:
    raise ValueError('Unset ambient Cargo/Rust overrides before capture: ' + ', '.join(unexpected))
# These match the explicit Cargo flags; record the actual environment supplied.
os.environ.update(CARGO_TARGET_DIR=str(target), CARGO_BUILD_JOBS='1')
effective = {**os.environ, 'RUSTFLAGS': command[3].split('=', 1)[1]}
effective.pop('CARGO_ENCODED_RUSTFLAGS', None)  # Applied by the explicit env argv.
sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
config = Path.cwd().resolve() / '.cargo/config.toml'
record = {'schema': 1, 'status': 'captured-before-exec-not-build-success',
          'root': str(Path.cwd().resolve()), 'command': command, 'capture_argv': sys.orig_argv,
          'capture_sha256': sha(__file__), 'cargo': {'path': command[4], 'sha256': sha(command[4])},
          'cargo_config': {'path': str(config), 'sha256': sha(config)},
          **{key: effective.get(key) for key in ('CARGO_TARGET_DIR', 'CARGO_BUILD_JOBS',
              'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTUP_TOOLCHAIN', 'RUSTC_WRAPPER',
              'RUSTC_WORKSPACE_WRAPPER', 'CARGO_BUILD_RUSTFLAGS')}}
with context_path.open('x') as handle:
    json.dump(record, handle, indent=2)
    handle.write('\n')
context_path.chmod(0o400)
os.execv(command[0], command)
