"""Serial pinned-toolchain vector capacity diagnostic; no rendering or timing."""
import hashlib,json,os,subprocess
from pathlib import Path
root=Path.cwd();local=root/'benchmarks/results/local/rectangle-reserve/capacity-002'
local.mkdir(exist_ok=False)
source=root/'benchmarks/results/sources/rectangle-reserve-rss/vec_capacity.rs'
sha=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
log=root/'benchmarks/results/local/rectangle-reserve/build-001/candidate.log'
artifacts=[json.loads(line) for line in log.read_text().splitlines() if line.startswith('{')]
libs=[p for a in artifacts if a.get('reason')=='compiler-artifact' and a.get('target',{}).get('name')=='pagura_raster' for p in a['filenames'] if p.endswith('.rlib')]
assert len(libs)==1
lib=Path(libs[0]);binary=local/'probe'
command=['rustc','--edition=2024','-C','lto=thin','-C','opt-level=3',str(source),'--extern','pagura_raster='+str(lib),'-L','dependency='+str(lib.parent),'-o',str(binary)]
report={'status':'incomplete','run_id':os.environ.get('PAGURA_RUN_ID'),'source_sha256':sha(source),'driver_sha256':sha(__file__),'rlib':str(lib),'rlib_sha256':sha(lib),'build_log_sha256':sha(log),'command':command,'toolchain':subprocess.check_output(['rustc','-Vv'],text=True)}
try:
 result=subprocess.run(command,capture_output=True,text=True,timeout=60)
 (local/'build.log').write_text(result.stdout+result.stderr)
 report['build_exit_code']=result.returncode
 result.check_returncode()
 raw=subprocess.check_output([str(binary)],timeout=10)
 (local/'output.jsonl').write_bytes(raw)
 report.update(status='completed-diagnostic',binary_sha256=sha(binary),output_sha256=sha(local/'output.jsonl'),records=[json.loads(line) for line in raw.splitlines()])
 print(json.dumps(report['records']))
finally:
 (local/'report.json').write_text(json.dumps(report,indent=2)+'\n')
