"""Build and retain one profiling binary from the committed product."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

root = Path.cwd()
local = root / 'benchmarks/results/local/n16-post-u16-profile'
command = ['cargo', 'build', '--locked', '--offline', '--release', '-p',
           'pagura-render', '--example', 'render_profile', '-j', '1']
environment = dict(os.environ, CARGO_BUILD_JOBS='1')
metadata = {'command': command, 'cargo': subprocess.check_output(['cargo', '-V'], text=True),
            'rustc': subprocess.check_output(['rustc', '-Vv'], text=True),
            'environment': {key: environment.get(key) for key in (
                'CARGO_BUILD_JOBS', 'CARGO_TARGET_DIR', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS',
                'CARGO_PROFILE_RELEASE_LTO', 'CARGO_PROFILE_RELEASE_CODEGEN_UNITS')}}
with (local / 'build-001.log').open('x') as log:
    subprocess.run(command, env=environment, stdout=log, stderr=subprocess.STDOUT, check=True)
target = Path(environment.get('CARGO_TARGET_DIR', root / 'target'))
source = target / 'release/examples/render_profile'
binary = local / 'render_profile'
assert not binary.exists()
shutil.copy2(source, binary)
binary.chmod(0o500)
metadata['binary_sha256'] = hashlib.sha256(binary.read_bytes()).hexdigest()
with (local / 'build-001.json').open('x') as output:
    json.dump(metadata, output, indent=2)
    output.write('\n')
print('Retained fresh profile binary:', metadata['binary_sha256'])
