import json,os,subprocess
from pathlib import Path
base=Path('benchmarks/results/local/n16-output-u16')
subprocess.check_call(['python3','benchmarks/opaque_precision_probe.py','--binary',str(base/'candidate'),'--output',str(base/'accuracy-001')])
assert json.loads((base/'accuracy-001/report.json').read_text())['all_within_contract']
subprocess.check_call(['node','benchmarks/opaque_host_probe.mjs','--outputdir',str(base/'host-001'),'--run-id',os.environ['PAGURA_RUN_ID']])
