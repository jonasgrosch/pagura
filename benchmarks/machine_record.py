"""Best-effort machine/process evidence; denied probes are explicit, never idle."""
import json
import os
import platform
import subprocess
from datetime import datetime, timezone


def probe(command):
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=10, env={**os.environ, "LC_ALL": "C"})
        return {"command": command, "status": "available" if result.returncode == 0 else "unavailable",
                "exit_code": result.returncode, "stdout": result.stdout, "stderr": result.stderr}
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"command": command, "status": "unavailable", "diagnostic": str(error)}


def snapshot(node="node"):
    record = {"utc": datetime.now(timezone.utc).isoformat(), "platform": platform.platform(),
              "architecture": platform.machine(), "logical_cores": os.cpu_count(),
              "load_average": list(os.getloadavg())}
    record["node"] = probe([node, "--single-threaded", "-e",
        'const o=require("node:os");console.log(JSON.stringify({cpus:o.cpus(),versions:process.versions}))'])
    if record["node"]["status"] == "available":
        value = json.loads(record["node"]["stdout"])
        record["cpu_model"] = value["cpus"][0]["model"] if value["cpus"] else "unavailable"
        record["cpu_times"] = [cpu["times"] for cpu in value["cpus"]]
        record["runtime_versions"] = value["versions"]
    record["power"] = probe(["pmset", "-g", "batt"]) if platform.system() == "Darwin" else {"status": "unavailable"}
    record["physical_cores"] = probe(["sysctl", "-n", "hw.physicalcpu"]) if platform.system() == "Darwin" else {"status": "unavailable"}
    record["os_version"] = probe(["sw_vers"]) if platform.system() == "Darwin" else {"status": "available", "stdout": platform.version()}
    record["processes"] = probe(["ps", "-A", "-o", "pid=,ppid=,pcpu=,comm="])
    record["heavy_processes"] = None
    if record["processes"]["status"] == "available":
        processes, children = [], {}
        for line in record["processes"]["stdout"].splitlines():
            fields = line.split(None, 3)
            if len(fields) != 4:
                raise ValueError("malformed process record")
            pid, parent, cpu = int(fields[0]), int(fields[1]), float(fields[2])
            processes.append({"pid": pid, "parent": parent, "cpu_percent": cpu, "command": fields[3]})
            children.setdefault(parent, []).append(pid)
        # Probes and their descendants belong to the harness, even if ps sees them busy.
        excluded, pending = set(), [os.getpid()]
        while pending:
            pid = pending.pop()
            if pid not in excluded:
                excluded.add(pid)
                pending.extend(children.get(pid, []))
        record["heavy_processes"] = [process for process in processes
            if process["pid"] not in excluded and process["cpu_percent"] >= 25]
    return record
