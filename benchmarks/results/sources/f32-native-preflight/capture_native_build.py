"""Capture one fixed native build context, then exec Cargo; no success inference."""
import hashlib
import json
import os
from pathlib import Path
import sys

if len(sys.argv) < 4 or sys.argv[2] != '--':
    raise ValueError('capture_native_build.py NEW_CONTEXT_JSON -- ABS_CARGO build ...')
context_path, command = Path(sys.argv[1]).resolve(), sys.argv[3:]
if (len(command) != 14 or not Path(command[0]).is_absolute()
        or command[1:9] != ['build', '--locked', '--offline', '--release', '-p', 'pagura-render',
                            '--example', 'scene_index_profile']
        or command[9] != '--target-dir'
        or command[11:] != ['-j', '1', '--message-format=json-render-diagnostics']):
    raise ValueError('Command is not the fixed native Cargo build')
target = Path(command[10])
if not target.is_absolute() or target != target.resolve():
    raise ValueError('Target directory must be canonical and absolute')
allowed = {'CARGO_TARGET_DIR', 'CARGO_BUILD_JOBS'}
unexpected = sorted(k for k, v in os.environ.items()
                    if v and k.startswith(('CARGO_', 'RUST')) and k not in allowed)
if unexpected:
    raise ValueError('Unset ambient Cargo/Rust overrides before capture: ' + ', '.join(unexpected))
os.environ.update(CARGO_TARGET_DIR=str(target), CARGO_BUILD_JOBS='1')
sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
config = Path.cwd().resolve() / '.cargo/config.toml'
record = {'schema': 1, 'status': 'captured-before-exec-not-build-success',
          'root': str(Path.cwd().resolve()), 'command': command, 'capture_argv': sys.orig_argv,
          'capture_sha256': sha(__file__), 'cargo': {'path': command[0], 'sha256': sha(command[0])},
          'cargo_config': {'path': str(config), 'sha256': sha(config)},
          **{k: os.environ.get(k) for k in ('CARGO_TARGET_DIR', 'CARGO_BUILD_JOBS', 'RUSTFLAGS',
              'CARGO_ENCODED_RUSTFLAGS', 'RUSTUP_TOOLCHAIN', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'CARGO_BUILD_RUSTFLAGS')}}
with context_path.open('x') as handle:
    json.dump(record, handle, indent=2)
    handle.write('\n')
context_path.chmod(0o400)
os.execv(command[0], command)
