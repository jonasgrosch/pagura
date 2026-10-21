import hashlib, json, os, sys, time
from pathlib import Path
root=Path.cwd(); local=root/'benchmarks/results/local/n16-output-u16'; results=root/'benchmarks/results'
sha=lambda path:hashlib.sha256(path.read_bytes()).hexdigest()
read=lambda name:json.loads((local/name).read_text())
full=read('full-check-001-receipt.json'); wasm=read('wasm-diagnostic-001-analysis.json'); pilot=read('pilot-001-analysis.json')
for name in ['full-check-001','wasm-diagnostic-001','wasm-diagnostic-001-audit','accuracy-001','pilot-001','pilot-001-audit']:
    receipt=read(name+'-receipt.json')
    assert receipt['status']=='completed' and receipt['source_unchanged']
    assert receipt['exit_code']==receipt['wrapper_exit_code']==0 and not receipt['errors'] and not receipt['artifact_errors']
    for path,digest in receipt['artifacts'].items():assert sha(Path(path))==digest
assert wasm['status']=='verified' and wasm['verified_outputs']==168 and not wasm['suspension_cells'] and not wasm['background_contention_observed']
assert pilot['independently_verified_outputs']==920 and not pilot['background_contention_observed']
assert any(c['role']=='target' and c['latency']['median_upper']<=.95 for c in pilot['cells'])
bindings=json.loads((results/'n16-output-u16-bindings.json').read_text())
for name,digest in bindings['bound_files_sha256'].items():assert sha(root/name)==digest
for identity in bindings['identities'].values():assert sha(Path(identity['binary']))==identity['binary_sha256']
assert full['source_before']['sha256']==bindings['candidate_runner_source_sha256']
assert read('accuracy-001/report.json')['all_within_contract'] and read('host-001/report.json')['status']=='passed'
decision={'decision':'launch-frozen-confirmation-once','created_unix_ns':time.time_ns(),'run_id':os.environ['PAGURA_RUN_ID'],
'reasons':['One target latency gate passes in the five-pair pilot, the other has a favorable median but misses its upper bound. All920 independent outputs pass.',
'Two protected pilot p95 lower bounds exceed1.02; every cell remains in confirmation and admission remains false.',
'Independent six-cell WASI diagnostic verifies168 outputs with unchanged linear-memory end sizes and no predeclared suspension signal. It establishes no portable speed admission.',
'Full repository validation and strict native/WASI precision probes completed. No source or sampling/gate change is authorized by this decision.'],
'pilot':{k:pilot[k] for k in ['target_latency_passes','protected_latency_passes','memory_passes']},
'pilot_supported_regressions':[c for c in pilot['cells'] if c['role']=='protected' and (c['latency']['median_lower']>1.02 or c['latency']['p95_lower']>1.02)],
'evidence_sha256':{name:sha(local/name) for name in ['pilot-001-analysis.json','wasm-diagnostic-001-analysis.json','wasm-diagnostic-001-audit-receipt.json','full-check-001-receipt.json','accuracy-001-receipt.json']},
'plan_sha256':sha(results/'n16-output-u16-plan.json'),'bindings_sha256':sha(results/'n16-output-u16-bindings.json'),
'experimental_restriction':'Experimental branch only. Preserve a correct partial win only under the frozen target median-benefit rule; full target latency, protected latency, RSS, release and competitor gates remain separate.'}
with (local/'confirmation-launch.json').open('x') as f:json.dump(decision,f,indent=2);f.write('\n')
args=['benchmarks/n16_output_u16.py','--baseline',str(local/'baseline'),'--candidate',str(local/'candidate'),'--baseline-source','/Users/jonasgrosch/.codex/worktrees/n16-conversion-control/pagura','--candidate-source',str(root),'--output',str(local/'confirmation-001'),'--phase','confirmation']
os.execv(sys.executable,[sys.executable,*args])
