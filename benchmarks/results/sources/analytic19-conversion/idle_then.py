"""Wait for a quiet local slot, then replace this process with one command."""
import json,os,sys,time
from pathlib import Path
from load_observer import snapshot,excluded_processes,prohibited,utc_now
out=Path(sys.argv[1]); command=sys.argv[2:]
started=time.monotonic(); quiet=0; scans=[]
while time.monotonic()-started < 3600:
    raw,rows=snapshot()
    excluded=excluded_processes(rows,None)
    busy=[{"pid":pid,"program":row["command"].split()[0],"reason":reason}
          for pid,row in rows.items() if pid not in excluded and (reason:=prohibited(row["command"]))]
    scans.append({"at":utc_now(),"busy":busy})
    quiet=quiet+1 if not busy else 0
    out.write_text(json.dumps({"command":command,"status":"launching" if quiet>=3 else "waiting","scans":scans},indent=2)+"\n")
    if quiet>=3:
        print("Quiet preflight passed; launching one owned job.",flush=True)
        os.execvp(command[0],command)
    if len(scans)==1 or len(scans)%12==0:
        print("Waiting for local compute:",len(busy),"known jobs",flush=True)
    time.sleep(5)
print("No quiet slot within the queue window; no job launched.",flush=True)
sys.exit(75)
