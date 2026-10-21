import hashlib, json, subprocess, sys
from pathlib import Path

root=Path.cwd()
local=root/'benchmarks/results/local/n16-output-u16'
src=local/'wasm-diagnostic-src'
sha=lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
receipt=local/'wasm-diagnostic-001-receipt.json'
guard=local/'wasm-diagnostic-001-observer/guard.json'
builds=local/'wasm-build-bindings.json'
native=root/'benchmarks/results/n16-output-u16-bindings.json'
node=Path('/opt/homebrew/bin/node').resolve(strict=True)
assert sha(guard)=='e19191796e67a6c4a211e7a649b3162df77a5191afd51beee8db014062bbd5af'
assert sha(native)=='741ed59e9ec1e9b6d25cf13747d3c9d5bc9de267f09848427e17f1a3512c3c26'
b=json.loads(builds.read_text())
assert b['baseline']['sha256']=='ecb2823b4f7b446ac7d4adcd8ffbf7a3d89516fd814455f91c0866eaf2091949'
assert b['candidate']['sha256']=='939001fbc3dbdd2f151550a08821893344d9b25d331c0627ad6e5e813264d423'
paths={'receipt':receipt,'guard':guard,'build-bindings':builds,'native-bindings':native,'driver':src/'diagnostic.py','wrapper':src/'run_profile_wasi.mjs','node':node}
pins={key:{'path':str(path),'sha256':sha(path)} for key,path in paths.items()}
with (local/'wasm-audit-pins.json').open('x') as f:json.dump(pins,f,indent=2);f.write('\n')
args=[sys.executable,str(src/'audit.py'),'--directory',str(local/'wasm-diagnostic-001'),'--receipt',str(receipt),'--guard',str(guard),'--build-bindings',str(builds),'--output',str(local/'wasm-diagnostic-001-analysis.json')]
for key,value in pins.items():args += ['--'+key+'-sha256',value['sha256']]
subprocess.run(args,check=True)
