import hashlib,json,os,sys
from pathlib import Path
root=Path.cwd();sys.path.insert(0,str(root/'benchmarks'));sys.path.insert(0,str(root/'tools'))
from paired_render import source_identity
from benchkit.config import source_digest
local=root/'benchmarks/results/local/n16-output-u16'
receipt_path=local/sys.argv[1]
receipt=json.loads(receipt_path.read_text())
assert receipt['status']=='completed' and receipt['source_unchanged']
assert receipt['exit_code']==receipt['wrapper_exit_code']==0
assert receipt['source_root']==str(root)
assert receipt['source_after']['sha256']==source_digest()
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
source=root/'target/release/examples/scene_index_profile'
assert receipt['artifacts'][str(source)]==sha(source)
candidate=local/'candidate'
with candidate.open('xb') as f:f.write(source.read_bytes())
candidate.chmod(0o500)
control=Path('/Users/jonasgrosch/.codex/worktrees/n16-conversion-control/pagura')
identities={side:{**source_identity(folder),'binary':str(local/side),'binary_sha256':sha(local/side)} for side,folder in [('baseline',control),('candidate',root)]}
files=['benchmarks/n16_output_u16.py','benchmarks/evaluate_n16_output_u16.py','benchmarks/n16_output_guarded.py','benchmarks/evaluate_n16_output_guarded.py','benchmarks/n16_output.py','benchmarks/evaluate_n16_output.py','benchmarks/opaque_composition.py','benchmarks/paired_render.py','benchmarks/glyph_memo.py','benchmarks/glyph_memo_fixtures.py','benchmarks/scene_index.py','benchmarks/clip_viewport_fixtures.py','crates/pagura-render/examples/scene_index_profile.rs','benchmarks/results/n16-output-u16-plan.json','benchmarks/results/local/glyph-memo/load_observer.py','benchmarks/results/local/glyph-memo/idle_then.py','tools/fixturegen/pdf.py','benchmarks/opaque_precision_probe.py','benchmarks/opaque_precision_fixtures.py','benchmarks/opaque_host_probe.mjs','benchmarks/results/local/n16-output-u16/check_and_build.py','benchmarks/results/local/n16-output-u16/accuracy.py','benchmarks/results/local/n16-output-u16/freeze.py']
old=root/'benchmarks/results/local/n16-output-guarded'
old_report=json.loads((old/'confirmation-001/report.json').read_text())
old_receipt=old/'full-check-001-receipt.json'
assert identities['baseline']['source_sha256']==old_report['identities']['candidate']['source_sha256']
assert sha(local/'baseline')==old_report['identities']['candidate']['binary_sha256']
baseline={'binary_sha256':sha(local/'baseline'),'receipt':str(old_receipt.relative_to(root/'benchmarks/results')),'receipt_sha256':sha(old_receipt),'reuse':'Prior guarded candidate became committedf98ad836 without changing its measured product fingerprint. Reused exact previously built binary; not an independent rebuild.','source':identities['baseline']}
assert baseline['binary_sha256']==identities['baseline']['binary_sha256']
bindings={'schema':1,'identities':identities,'bound_files_sha256':{name:sha(root/name) for name in files},'candidate_runner_source_sha256':source_digest(),'candidate_build_receipt':str(receipt_path.relative_to(root/'benchmarks/results')),'candidate_build_receipt_sha256':sha(receipt_path),'baseline_build':baseline,'validation':'Exhaustive all-u16 conversion, background source-over regression in both modes/full/tiles, focused native regression tests, renderer clippy and native/WASM host builds. Independent precision/parity follows freeze; complete repository check is required before retention.'}
path=root/'benchmarks/results/n16-output-u16-bindings.json'
with path.open('x') as f:json.dump(bindings,f,indent=2);f.write('\n')
print(json.dumps(identities,indent=2))
