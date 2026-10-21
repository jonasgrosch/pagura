"""Install/remove only the exact R2 trace and prove byte-for-byte restoration."""
import hashlib,json,subprocess,sys
from pathlib import Path
root=Path.cwd().resolve()
sys.path[:0]=[str(root/'benchmarks'),str(root/'tools')]
from paired_render import source_identity
from benchkit.config import source_digest
local=root/'benchmarks/results/local/n16-painted-interval'
folder=local/'reach-r2-preparation-001'
sources=root/'benchmarks/results/sources/n16-painted-interval-reach-r2'
patch=sources/'reach.patch'
files=('crates/pagura-render/src/compositor.rs','crates/pagura-render/src/compositor/narrow.rs','crates/pagura-render/src/compositor/output.rs','crates/pagura-render/examples/scene_index_profile.rs')
def sha(p):return hashlib.sha256(Path(p).read_bytes()).hexdigest()
def pin(p):return {'path':str(p),'sha256':sha(p)}
def git(*args):return subprocess.check_output(['git','-C',str(root),*args],text=True)
def snapshot():return {'commit':git('rev-parse','HEAD').strip(),'sha256':source_digest()}
def write(p,v):
 with p.open('x') as f:json.dump(v,f,indent=2);f.write('\n')
if sys.argv[1]=='install':
 folder.mkdir(exist_ok=False)
 closure=set(root.glob('benchmarks/*.py'))|set((root/'tools/fixturegen').rglob('*.py'))|set(sources.glob('*'))
 closure={str(p):sha(p) for p in closure if p.is_file()}
 rec={'schema':2,'revision':'R2','pre_probe_source':source_identity(root),'pre_runner_source':snapshot(),'patch':pin(patch),'files_before':{p:sha(root/p) for p in files},'helper_sha256':closure,'prior_failed_reach':pin(local/'reach-001/report.json'),'scope':'Fresh 24-output structural revision; original R1 remains failed, timing matrix and gates unchanged.'}
 assert rec['pre_probe_source']['source_sha256']=='53062678753e6012a291b95dbf07cb8b069e7134ab836d587919975d1ed057c7'
 write(folder/'before.json',rec)
 git('apply','--check',str(patch));git('apply',str(patch))
 rec.update(instrumented_source=source_identity(root),instrumented_runner_source=snapshot(),files_installed={p:sha(root/p) for p in files})
 write(folder/'installed.json',rec)
 print(json.dumps(rec['instrumented_source']))
elif sys.argv[1]=='bind':
 rec=json.loads((folder/'installed.json').read_text());assert source_identity(root)==rec['instrumented_source']
 binary=Path('/private/tmp/n16-interval-reach-r2-target-001/release/examples/scene_index_profile')
 receipts=[root/'benchmarks/results/local/main-reconciliation/n16-reach-r2-build-001.json',local/'reach-r2-build-001-receipt.json',local/'reach-r2-build-001-observer/guard.json']
 receipt=json.loads(receipts[1].read_text());assert receipt['status']=='completed' and receipt['source_unchanged'] and receipt['source_before']==rec['instrumented_runner_source'] and receipt['exit_code']==receipt['wrapper_exit_code']==0 and not receipt['errors'] and not receipt['artifact_errors']
 assert receipt['artifacts'][str(binary)]==sha(binary)
 preparation={**rec,'binary':pin(binary),'build_evidence':[pin(p) for p in receipts]}
 write(folder/'preparation.json',preparation);print(sha(folder/'preparation.json'))
elif sys.argv[1]=='remove':
 rec=json.loads((folder/'installed.json').read_text());assert source_identity(root)==rec['instrumented_source'] and snapshot()==rec['instrumented_runner_source']
 assert sha(patch)==rec['patch']['sha256'] and {p:sha(root/p) for p in files}==rec['files_installed']
 git('apply','--reverse','--check',str(patch));git('apply','--reverse',str(patch))
 restored=source_identity(root);after=snapshot();hashes={p:sha(root/p) for p in files}
 assert restored==rec['pre_probe_source'] and after==rec['pre_runner_source'] and hashes==rec['files_before']
 assert all(sha(p)==h for p,h in rec['helper_sha256'].items())
 out={'schema':2,'revision':'R2','preparation':pin(folder/'installed.json'),'restored':True,'pre_probe_source':rec['pre_probe_source'],'source_after':restored,'runner_source_after':after,'files_before':rec['files_before'],'files_installed':rec['files_installed'],'files_after':hashes,'helper_closure_unchanged':True}
 write(folder/'removed.json',out);print(json.dumps(out,indent=2))
else:raise ValueError('install, bind or remove')
