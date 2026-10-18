"""Enforce the product dependency boundary and evidence manifest integrity."""
import argparse
import hashlib
import json
import re
import subprocess
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TAG = re.compile(r"^[a-z0-9]+\.[A-Za-z0-9-]+$")
# Content operators keep their PDF spelling, punctuation included (Annex A).
OPERATOR_TAG = re.compile(r"^operator\.[A-Za-z0-9*'\"]+$")


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def check_product() -> int:
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())
    assert workspace["workspace"]["lints"]["rust"]["unsafe_code"] == "forbid"
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--offline", "--format-version", "1"], cwd=ROOT))
    members = set(metadata["workspace_members"])
    assert members and {p["id"] for p in metadata["packages"]} == members, "external product dependency"
    for package in metadata["packages"]:
        assert package["source"] is None and not package["links"], "native/external package"
        manifest = Path(package["manifest_path"])
        assert manifest.is_relative_to(ROOT / "crates"), "product must be in crates/"
        config = tomllib.loads(manifest.read_text())
        assert config["lints"]["workspace"] is True, f"{manifest}: workspace lint inheritance missing"
        assert not (manifest.parent / "build.rs").exists(), "product build script prohibited"
        for target in package["targets"]:
            assert "custom-build" not in target["kind"] and "proc-macro" not in target["kind"]
            source = Path(target["src_path"]).read_text()
            assert "#![forbid(unsafe_code)]" in source, f"{target['src_path']}: lacks unsafe prohibition"
    return len(members)


def check_fixtures() -> int:
    requirements = set()
    for path in sorted((ROOT / "references/requirements").glob("*.json")):
        for requirement in json.loads(path.read_text())["requirements"]:
            assert requirement["id"] not in requirements, f"duplicate requirement {requirement['id']}"
            requirements.add(requirement["id"])
    cases = 0
    for path in sorted((ROOT / "corpus/manifests").glob("*.json")):
        manifest = json.loads(path.read_text())
        assert manifest["area"] == path.stem, f"{path}: area does not match file name"
        for case in manifest["cases"]:
            cases += 1
            assert set(case["requirements"]) <= requirements, f"{case['id']}: unmapped requirement"
            if case.get("feature"):
                assert TAG.match(case["feature"]) or OPERATOR_TAG.match(case["feature"]), f"{case['id']}: malformed feature tag"
            for tag in [*case.get("repairs", []), *case.get("information", [])]:
                assert TAG.match(tag), f"{case['id']}: malformed diagnostic tag"
            files = [(case["path"], case["sha256"])] if case["kind"] == "pdf" else []
            if "oracle" in case:
                files.append((case["oracle"], case["oracle_sha256"]))
            files += [(f["path"], f["sha256"]) for f in case.get("files", {}).values()]
            for relative, digest in files:
                target = ROOT / relative
                assert target.resolve().is_relative_to(ROOT / "corpus/generated"), "escaping fixture path"
                assert sha256(target) == digest, f"{relative}: changed fixture"
    return cases


def check_local_assets() -> None:
    lock = json.loads((ROOT / "references/lock.json").read_text())
    for reference in lock["sources"]:
        if reference["status"] in ["cached", "locally-supplied"]:
            from references import CACHE, cached_path
            path = cached_path(reference["cache_path"])
            assert path.resolve().is_relative_to(ROOT) or path.resolve().is_relative_to(CACHE), "escaping reference path"
            assert sha256(path) == reference["sha256"], "changed reference"
    from corpus import verify
    verify()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--local-assets", action="store_true")
    arguments = parser.parse_args()
    packages = check_product()
    cases = check_fixtures()
    if arguments.local_assets:
        check_local_assets()
    print(f"Policy passed: {packages} workspace-only packages, unsafe forbidden, {cases} fixture cases verified")
