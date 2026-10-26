"""Bounded JSON/byte-hash bridge for the isolated hayro public-API adapter.

All PDF traversal and stream decoding stay in hayro. This process only validates
the adapter envelope and hashes its observed bytes with the standard library.
"""
import hashlib
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

MAX_RECORDS = 128
MAX_PAYLOAD = 65536
MAX_TRANSFER = 1024 * 1024


def failure(status, reason):
    return {"schema": 1, "status": status, "reason": reason}


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def normalize(value, operation):
    require(isinstance(value, dict) and type(value.get("schema")) is int and value["schema"] == 1,
            "invalid hayro adapter schema")
    status = value.get("status")
    if status != "ok":
        require(status in {"rejected", "adapter-error", "limit"} and set(value) == {"schema", "status", "reason"}
                and isinstance(value["reason"], str) and bool(value["reason"].strip()), "invalid hayro adapter failure")
        return value
    require(set(value) == {"schema", "status", "value"}, "invalid hayro adapter success")
    if operation not in {"attachments", "annotation-attachments"}:
        return value
    records = value["value"]
    require(isinstance(records, list), "invalid hayro attachment array")
    if len(records) > MAX_RECORDS:
        return failure("limit", "hayro attachment count exceeds 128 adapter bound")
    output, total = [], 0
    for record in records:
        require(isinstance(record, dict) and set(record) == {"name", "bytes_hex"}, "invalid hayro attachment record")
        name, encoded = record["name"], record["bytes_hex"]
        require(isinstance(name, str) and name.isascii(), "hayro filename is outside ASCII adapter scope")
        if len(name) > 1024:
            return failure("limit", "hayro filename exceeds 1024 adapter bound")
        require(isinstance(encoded, str) and len(encoded) % 2 == 0 and re.fullmatch("[0-9a-f]*", encoded) is not None,
                "invalid hayro payload encoding")
        total += len(encoded) // 2
        if total > MAX_PAYLOAD:
            return failure("limit", "hayro decoded output exceeds 64 KiB adapter bound")
        output.append({"name": name, "sha256": hashlib.sha256(bytes.fromhex(encoded)).hexdigest()})
    output.sort(key=lambda item: (item["name"], item["sha256"]))
    return {"schema": 1, "status": "ok", "value": output}


def execute(binary, arguments):
    with tempfile.TemporaryDirectory(prefix="pagura-hayro-probe-") as temporary:
        directory = Path(temporary)
        stdout, stderr = directory / "stdout", directory / "stderr"
        try:
            with stdout.open("wb") as out, stderr.open("wb") as err:
                process = subprocess.run([binary, *arguments], stdout=out, stderr=err, timeout=15)
        except FileNotFoundError:
            return None, failure("engine-unavailable", "hayro adapter binary could not start")
        except subprocess.TimeoutExpired:
            return None, failure("timeout", "15 second hayro adapter deadline")
        if stdout.stat().st_size > MAX_TRANSFER or stderr.stat().st_size > 65536:
            return None, failure("limit", "hayro adapter transfer bound exceeded")
        if process.returncode:
            digest = hashlib.sha256(stderr.read_bytes()).hexdigest()
            return None, failure("adapter-error", f"hayro adapter exit {process.returncode}; stderr SHA-256 {digest}")
        return json.loads(stdout.read_bytes()), None


def main():
    args = sys.argv[1:]
    require(len(args) in {2, 3}, "expected BINARY OPERATION SYNTHETIC_PDF, or BINARY --version")
    version = args[1:] == ["--version"]
    require(version or len(args) == 3, "missing synthetic PDF path")
    try:
        value, error = execute(args[0], args[1:])
        if error is not None:
            result = error
        elif version:
            require(isinstance(value, dict) and set(value) == {"engine", "wrapper"}, "invalid hayro version record")
            result = value
        else:
            result = normalize(value, args[1])
        if len(json.dumps(result).encode()) > 65536:
            result = failure("limit", "hayro normalized report exceeds 64 KiB adapter bound")
    except (OSError, ValueError, TypeError):
        result = failure("adapter-error", "hayro adapter output could not be normalized")
    print(json.dumps(result))


if __name__ == "__main__":
    main()
