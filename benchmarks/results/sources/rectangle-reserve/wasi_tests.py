"""Execute both frozen WASI test modules serially with fixture access."""
import hashlib
import json
from pathlib import Path
import subprocess

root = Path.cwd()
bindings = json.loads((root / 'benchmarks/results/local/rectangle-reserve/wasi-modules.json').read_text())
sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
assert sha(bindings['runner']) == bindings['runner_sha256']
assert len(bindings['modules']) == 2
for module in bindings['modules']:
    assert sha(module['path']) == module['sha256']
    subprocess.run(['node', bindings['runner'], module['path'], str(root), 'reserve_tests'],
                   check=True, timeout=120)
    assert sha(module['path']) == module['sha256']
assert sha(bindings['runner']) == bindings['runner_sha256']
