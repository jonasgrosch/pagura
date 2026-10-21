"""Shared paths and source provenance."""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
POLICY_PATH = ROOT / "benchmarks/policy.json"
MANIFESTS = ROOT / "corpus/manifests"
MATRIX_PATH = ROOT / "benchmarks/release-matrix.json"
ADAPTERS = ROOT / "benchmarks/adapters"


def sha(data):
    return hashlib.sha256(data).hexdigest()


def read_json(path):
    return json.loads(Path(path).read_text())


def source_digest():
    paths = set()
    for directory in ["crates", "tools", "benchmarks/adapters"]:
        paths.update(p for p in (ROOT / directory).rglob("*") if p.is_file()
                     and "__pycache__" not in p.parts and "target" not in p.parts)
    paths.update(ROOT / p for p in ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml",
                                   "benchmarks/policy.json", "benchmarks/release-matrix.json",
                                   ".python-version", "pyproject.toml", "uv.lock",
                                   "benchmarks/node-package.json", "benchmarks/node-package-lock.json"])
    paths.update(MANIFESTS.glob("*.json"))
    return sha(b"".join(str(p.relative_to(ROOT)).encode() + b"\0" + p.read_bytes() for p in sorted(paths) if p.exists()))

