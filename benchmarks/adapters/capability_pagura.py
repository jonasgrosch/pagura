"""G01 adapter over the public Pagura CLI; no internal PDF traversal or oracle data.

The caller supplies a built CLI, an operation and a synthetic input path. Each
operation returns the same bounded envelope as the external-engine adapters.
"""
import hashlib
import json
import math
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

MAX_OUTPUT = 65536
MAX_PAYLOAD = 64 * 1024 * 1024


class ProbeFailure(Exception):
    def __init__(self, status, reason, **fields):
        super().__init__(reason)
        self.response = {"status": status, "reason": reason, **fields}


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def failure(code, stderr):
    """Only exit 1 plus a known engine-error prefix establishes a refusal.

    Crashes, usage errors, IO errors and unknown messages stay adapter failures.
    Native diagnostics are summarized by hash, never copied into reports.
    """
    digest = hashlib.sha256(stderr).hexdigest()
    if code == 1:
        match = re.match(rb"pagura: unsupported \[([A-Za-z0-9.-]+)\]: ", stderr)
        if match:
            return ProbeFailure("unsupported", f"Pagura explicit refusal; stderr SHA-256 {digest}",
                                feature=match[1].decode("ascii"))
        if stderr.startswith(b"pagura: limit exceeded: "):
            return ProbeFailure("limit", f"Pagura resource refusal; stderr SHA-256 {digest}")
        if re.match(rb"pagura: (syntax error|missing object|cycle|password required): ", stderr):
            return ProbeFailure("rejected", f"Pagura input rejection; stderr SHA-256 {digest}")
    return ProbeFailure("adapter-error", f"unclassified CLI exit {code}; stderr SHA-256 {digest}")


def execute(binary, arguments, directory, deadline):
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise ProbeFailure("timeout", "20 second subject-operation deadline")
    stdout, stderr = directory / "cli-stdout", directory / "cli-stderr"
    try:
        with stdout.open("wb") as out, stderr.open("wb") as err:
            result = subprocess.run([binary, *arguments], stdout=out, stderr=err, timeout=remaining)
    except subprocess.TimeoutExpired as error:
        raise ProbeFailure("timeout", "20 second subject-operation deadline") from error
    except FileNotFoundError as error:
        raise ProbeFailure("engine-unavailable", "Pagura CLI could not start") from error
    if max(stdout.stat().st_size, stderr.stat().st_size) > MAX_OUTPUT:
        raise ProbeFailure("limit", "subject command output exceeds 64 KiB adapter bound")
    if result.returncode:
        raise failure(result.returncode, stderr.read_bytes())
    return stdout.read_bytes()


def report(binary, command, source, directory, deadline, schema=1):
    value = json.loads(execute(binary, [command, source], directory, deadline))
    require(isinstance(value, dict) and type(value.get("schema")) is int
            and value["schema"] == schema, "unexpected CLI report schema")
    return value


def page_sizes(value):
    count, pages = value.get("pages"), value.get("page_geometry")
    require(type(count) is int and 0 <= count <= 1024 and isinstance(pages, list)
            and len(pages) == count, "invalid page count/geometry")
    sizes = []
    for page in pages:
        require(isinstance(page, dict), "invalid page geometry")
        box, rotation, unit = page.get("crop_box"), page.get("rotation"), page.get("user_unit")
        require(isinstance(box, list) and len(box) == 4
                and all(type(x) in (float, int) and math.isfinite(x) for x in box), "invalid crop box")
        require(type(rotation) is int and rotation in (0, 90, 180, 270)
                and type(unit) in (int, float) and math.isfinite(unit) and unit > 0, "invalid page transform")
        width, height = (box[2] - box[0]) * unit, (box[3] - box[1]) * unit
        require(math.isfinite(width) and math.isfinite(height) and min(width, height) >= 0, "invalid page dimensions")
        sizes.append([height, width] if rotation in (90, 270) else [width, height])
    return {"count": count, "sizes": sizes}


def metadata(value):
    info = value.get("info")
    require("info" in value and (info is None or isinstance(info, dict)), "missing/invalid Info report")
    if info is None:
        return {"Title": "", "Author": ""}
    result = {}
    for key in ("Title", "Author"):
        require(key in info and (info[key] is None or isinstance(info[key], str)), "invalid Info field")
        result[key] = info[key] if info[key] is not None else ""
    return result


def attachments(binary, value, source, directory, deadline):
    files = value.get("files")
    require(isinstance(files, list) and len(files) <= 1024, "invalid attachment list")
    result = []
    for index, file in enumerate(files):
        require(isinstance(file, dict) and type(file.get("index")) is int
                and file["index"] == index, "invalid attachment index")
        names, payloads = file.get("names"), file.get("payloads")
        if not isinstance(names, dict) or not isinstance(payloads, list):
            raise ValueError("invalid attachment fields")
        name = names.get("UF")
        if name is None:
            require(isinstance(names.get("F_hex"), str), "missing filename")
            # Current baseline fixtures use ASCII paths. Do not guess an
            # encoding for raw legacy filenames outside that measured scope.
            name = bytes.fromhex(names["F_hex"]).decode("ascii")
        require(isinstance(name, str), "invalid Unicode filename")
        require(all(isinstance(row, dict) and row.get("key") in ("F", "UF", "DOS", "Mac", "Unix")
                    for row in payloads), "invalid EF choices")
        keys = [row["key"] for row in payloads]
        require(len(keys) == len(set(keys)), "duplicate EF choices")
        key = "UF" if "UF" in keys else "F"
        require(key in keys, "no supported EF key in attachment descriptor")
        output = directory / "attachment.bin"
        output.unlink(missing_ok=True)
        execute(binary, ["extract-attachment", source, str(index), key, str(output)], directory, deadline)
        require(output.is_file(), "CLI did not create an extracted payload")
        if output.stat().st_size > MAX_PAYLOAD:
            raise ProbeFailure("limit", "attachment exceeds 64 MiB adapter bound")
        result.append({"name": name, "sha256": hashlib.sha256(output.read_bytes()).hexdigest()})
    return sorted(result, key=lambda row: row["name"])


def probe(binary, operation, source):
    deadline = time.monotonic() + 20
    try:
        with tempfile.TemporaryDirectory(prefix="pagura-service-adapter-") as temporary:
            directory = Path(temporary)
            if operation == "pages":
                value = page_sizes(report(binary, "inspect", source, directory, deadline, schema=2))
            elif operation == "metadata":
                value = metadata(report(binary, "metadata", source, directory, deadline))
            elif operation == "attachments":
                value = attachments(binary, report(binary, "attachments", source, directory, deadline),
                                    source, directory, deadline)
            elif operation == "text":
                value = execute(binary, ["text", source, "0"], directory, deadline).decode("utf-8").replace("\r\n", "\n").strip("\n")
            else:
                raise ValueError("unknown operation")
            return {"schema": 1, "status": "ok", "value": value}
    except ProbeFailure as error:
        return {"schema": 1, **error.response}
    except (ValueError, KeyError, TypeError, OSError) as error:
        return {"schema": 1, "status": "adapter-error", "reason": type(error).__name__}


if __name__ == "__main__":
    if len(sys.argv) != 4:
        raise SystemExit("expected CLI OPERATION SYNTHETIC_PDF")
    print(json.dumps(probe(*sys.argv[1:]), sort_keys=True))
