"""Apply/reverse the two exact diagnostic patches and record source restoration."""
import hashlib,json,subprocess,sys
from pathlib import Path
root=Path('/Users/jonasgrosch/.codex/worktrees/renderer-n16-interval/pagura')
sys.path.insert(0,str(root/'benchmarks'))
from paired_render import source_identity
folder=root/'benchmarks/results/local/n16-painted-interval/diagnostic-preparation-003'
roots={'baseline':root.parent.parent/'renderer-n16-interval-base/pagura','candidate':root}
patchdir=root/'benchmarks/results/sources/n16-painted-interval-validation'
patches=[patchdir/'layout-probe.patch',patchdir/'pi-q4-diagnostic.patch']
files=('crates/pagura-render/src/compositor.rs','crates/pagura-render/src/display.rs','crates/pagura-render/src/compositor/sources_tests.rs','crates/pagura-render/src/compositor/translucency_tests.rs')
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def git(r,*args):return subprocess.run(['git','-C',str(r),*args],check=True,capture_output=True,text=True).stdout
if sys.argv[1]=='install':
 folder.mkdir(exist_ok=False)
 assert git(roots['baseline'],'status','--porcelain=v1','--untracked-files=all')==''
 rec={'schema':1,'roots':{a:{'before':source_identity(r),'files':{p:sha(r/p) for p in files},'status_before':git(r,'status','--porcelain=v1','--untracked-files=all')} for a,r in roots.items()},'patches':[{'path':str(p),'sha256':sha(p)} for p in patches],'reason':'Fresh preparation after lint-only correction in two test assertions; isolated per-arm/per-platform diagnostics.'}
 (folder/'before.json').write_text(json.dumps(rec,indent=2)+'\n')
 for a,r in roots.items():
  for p in patches:
   git(r,'apply','--check',str(p));git(r,'apply',str(p))
  rec['roots'][a]['installed']=source_identity(r)
 (folder/'installed.json').write_text(json.dumps(rec,indent=2)+'\n')
 print(sha(folder/'installed.json'))
elif sys.argv[1]=='remove':
 rec=json.loads((folder/'installed.json').read_text())
 restored={}
 for a,r in roots.items():
  assert source_identity(r)==rec['roots'][a]['installed']
  for p in reversed(patches):
   assert sha(p)==next(i['sha256'] for i in rec['patches'] if i['path']==str(p))
   git(r,'apply','--reverse','--check',str(p));git(r,'apply','--reverse',str(p))
  assert {p:sha(r/p) for p in files}==rec['roots'][a]['files']
  restored[a]=source_identity(r)
  assert restored[a]==rec['roots'][a]['before']
 out={'schema':1,'preparation_sha256':sha(folder/'installed.json'),'removed':restored,'restored':True}
 (folder/'removed.json').write_text(json.dumps(out,indent=2)+'\n')
 print(json.dumps(out,indent=2))
else:raise ValueError('install or remove required')
