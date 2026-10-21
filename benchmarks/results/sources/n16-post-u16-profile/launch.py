import os,sys
from pathlib import Path
local=Path.cwd()/'benchmarks/results/local/n16-post-u16-profile'
os.execv(sys.executable,[sys.executable,'-B',str(local/'profile.py'),'--plan',str(local/'profile-plan.json'),'--plan-sha256',(local/'profile-plan.sha256').read_text().strip(),'--output',str(local/'profile-001')])
