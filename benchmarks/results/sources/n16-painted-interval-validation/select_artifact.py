#!/usr/bin/env python3
"""Archive the one successful Cargo-selected renderer test executable."""
import hashlib
import json
from pathlib import Path
import shutil
import sys

log, destination = (Path(value) for value in sys.argv[1:])
messages = [json.loads(line) for line in log.read_text().splitlines() if line.strip()]
finished = [item for item in messages if item.get("reason") == "build-finished"]
if len(finished) != 1 or finished[0].get("success") is not True:
    raise ValueError("Cargo did not record one successful completed build")
artifacts = [item for item in messages
             if item.get("reason") == "compiler-artifact"
             and item.get("target", {}).get("name") == "pagura_render"
             and item["target"].get("kind") == ["lib"]
             and item.get("profile", {}).get("test") is True
             and item.get("executable")]
if len(artifacts) != 1 or not destination.is_absolute():
    raise ValueError("Expected one renderer test artifact and an absolute destination")
source = Path(artifacts[0]["executable"]).resolve(strict=True)
if not source.is_file():
    raise ValueError("Selected artifact is not a regular file")
with source.open("rb") as reader, destination.open("xb") as writer:
    shutil.copyfileobj(reader, writer)
shutil.copymode(source, destination)


def digest(path):
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


sha256 = digest(destination)
if sha256 != digest(source):
    raise ValueError("Selected artifact changed while being archived")
metadata = {"schema": 1, "cargo_artifact": artifacts[0], "source": str(source),
            "retained": str(destination), "bytes": destination.stat().st_size,
            "sha256": sha256}
with destination.with_suffix(destination.suffix + ".json").open("x") as handle:
    json.dump(metadata, handle, indent=2)
    handle.write("\n")
print(destination)
