"""Regenerate or verify independent fixtures from every tools/fixturegen area.

Each area writes corpus/generated/<area>/ and corpus/manifests/<area>.json.
`--check` fails on any drift and never blesses changes.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib
import json
import pkgutil
from pathlib import Path

import fixturegen
from fixturegen.competitive import BOUNDS as COMPETITIVE_BOUNDS
from fixturegen.model import RENDER, UNSUPPORTED, Case, Vector

ROOT = Path(__file__).resolve().parents[1]
GENERATED = ROOT / "corpus/generated"
MANIFESTS = ROOT / "corpus/manifests"


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def ppm_size(data: bytes) -> tuple[int, int]:
    fields = data.split(maxsplit=4)
    if fields[0] != b"P6":
        raise SystemExit("oracles must be binary PPM")
    return int(fields[1]), int(fields[2])


def area_modules():
    for info in sorted(pkgutil.iter_modules(fixturegen.__path__), key=lambda m: m.name):
        module = importlib.import_module(f"fixturegen.{info.name}")
        if hasattr(module, "AREA") and hasattr(module, "cases"):
            yield module


def render_area(module) -> dict[Path, bytes]:
    area, files, entries = module.AREA, {}, []
    directory = GENERATED / area
    for case in module.cases():
        if isinstance(case, Case):
            entry = {"id": f"{area}/{case.name}", "kind": "pdf", "description": case.description,
                     "expected": case.expected, "requirements": list(case.requirements),
                     "path": f"corpus/generated/{area}/{case.name}.pdf", "sha256": sha256(case.pdf),
                     "pages": case.pages, "strict_valid": case.strict_valid, "repairs": list(case.repairs)}
            if case.information:
                entry["information"] = list(case.information)
            files[directory / f"{case.name}.pdf"] = case.pdf
            if case.expected == RENDER:
                if case.oracle is None:
                    raise SystemExit(f"{entry['id']}: render cases need an oracle")
                files[directory / f"{case.name}.ppm"] = case.oracle
                width, height = ppm_size(case.oracle)
                entry.update(oracle=f"corpus/generated/{area}/{case.name}.ppm",
                             oracle_sha256=sha256(case.oracle), dpi=case.dpi, width=width, height=height)
            if case.expected == UNSUPPORTED:
                if not case.feature:
                    raise SystemExit(f"{entry['id']}: unsupported cases need a feature tag")
                entry["feature"] = case.feature
        elif isinstance(case, Vector):
            entry = {"id": f"{area}/{case.name}", "kind": "vector", "description": case.description,
                     "requirements": list(case.requirements), "files": {}}
            for suffix, data in sorted(case.files.items()):
                path = directory / f"{case.name}{suffix}"
                files[path] = data
                entry["files"][suffix] = {"path": path.relative_to(ROOT).as_posix(), "sha256": sha256(data)}
        else:
            raise SystemExit(f"{area}: unknown case type {type(case).__name__}")
        entry.update(case.extra)
        if entry["id"] in COMPETITIVE_BOUNDS:
            if entry.get("expected") != RENDER:
                raise SystemExit(f"{entry['id']}: competitive bounds apply to render cases")
            entry["competitive_tolerance"] = COMPETITIVE_BOUNDS[entry["id"]]
        entries.append(entry)
    if len({e["id"] for e in entries}) != len(entries):
        raise SystemExit(f"{area}: duplicate case names")
    manifest = {"schema": 2, "area": area, "source": "Pagura independently authored fixtures",
                "license": "MIT OR Apache-2.0 (project-owned)", "cases": entries}
    files[MANIFESTS / f"{area}.json"] = (json.dumps(manifest, indent=2) + "\n").encode()
    return files


def main(check: bool) -> None:
    expected: dict[Path, bytes] = {}
    for module in area_modules():
        expected.update(render_area(module))
    drift = [p for p, data in expected.items() if not p.exists() or p.read_bytes() != data]
    owned_dirs = {p.parent for p in expected}
    stale = [p for d in owned_dirs if d.exists() for p in d.iterdir() if p.is_file() and p not in expected]
    if check:
        problems = [f"drift: {p.relative_to(ROOT)}" for p in drift] + [f"stale: {p.relative_to(ROOT)}" for p in stale]
        if problems:
            raise SystemExit("Fixture check failed (regeneration requires review):\n  " + "\n  ".join(problems))
    else:
        for path, data in expected.items():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        for path in stale:
            path.unlink()
    listed = {json.loads(data)["area"] + "/" for p, data in expected.items() if p.parent == MANIFESTS}
    unknown = [name for name in COMPETITIVE_BOUNDS if not any(name.startswith(area) for area in listed)]
    if unknown:
        raise SystemExit(f"competitive bounds name unknown areas: {unknown}")
    cases = sum(1 for p in expected if p.parent == MANIFESTS)
    print(f"{'Verified' if check else 'Generated'} fixtures for {cases} areas, {len(expected)} files")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    main(parser.parse_args().check)
