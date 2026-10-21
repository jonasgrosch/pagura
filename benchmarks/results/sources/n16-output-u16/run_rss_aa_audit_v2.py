import json,os,sys
from pathlib import Path
local=Path.cwd()/'benchmarks/results/local/n16-output-u16'
pins=json.loads((local/'rss-aa-audit-pins.json').read_text())
args=[str(local/'rss-aa-src/audit-v2.py'),'--directory',str(local/'rss-aa-001'),'--output',str(local/'rss-aa-001-analysis.json')]
for name,record in pins.items():
    if name!='bindings':args.extend(['--'+name,record['path']])
    args.extend(['--'+name+'-sha256',record['sha256']])
os.execv(sys.executable,[sys.executable,'-B',*args])
