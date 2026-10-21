import os, subprocess, sys
from pathlib import Path
root=Path.cwd();local=root/'benchmarks/results/local/n16-output-u16';src=local/'rss-aa-src'
with (local/'rss-aa-tests-001.log').open('x') as log:
    subprocess.run([sys.executable,'-B','-m','unittest','-v','test_rss_aa'],cwd=src,stdout=log,stderr=subprocess.STDOUT,check=True)
args=[str(src/'rss_aa.py'),'--root',str(root),'--bindings',str(root/'benchmarks/results/n16-output-u16-bindings.json'),'--binary',str(local/'candidate'),'--output',str(local/'rss-aa-001')]
os.execv(sys.executable,[sys.executable,'-B',*args])
