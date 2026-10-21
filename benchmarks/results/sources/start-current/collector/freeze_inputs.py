#!/usr/bin/env python3
"""Freeze all 35 independent PDF/PPM sources before either renderer is invoked."""
import argparse
import sys
from pathlib import Path

from protocol import cells, encoded, input_key, require, sha


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root, output = args.root.resolve(), args.output.resolve()
    sys.dont_write_bytecode = True
    sys.path[:0] = [str(root / "tools"), str(root / "benchmarks")]
    from benchkit.artifacts import hash_regular_file
    from benchkit.images import normalize
    import paired_render
    import opaque_composition as opaque
    import color_reuse_fixtures as analytic
    require(paired_render.ROOT.resolve() == root, "wrong helper root")
    output.mkdir(parents=True, exist_ok=False)
    matrix, inputs = cells(opaque, analytic), {}
    for cell in matrix:
        key = input_key(cell)
        if key in inputs:
            continue
        pdf, ppm = (opaque if cell["suite"] == "opaque" else analytic).fixture(cell["case"])
        width, height, _ = normalize(ppm)
        item = {"width": width, "height": height}
        for kind, raw, extension in (("pdf", pdf, "pdf"), ("oracle", ppm, "ppm")):
            path = output / kind / cell["suite"] / (cell["case"] + "." + extension)
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open("xb") as handle:
                handle.write(raw)
            path.chmod(0o400)
            item[kind] = {"path": str(path), "sha256": sha(raw), "bytes": len(raw)}
        inputs[key] = item
    require(len(inputs) == 35, "fixed input set differs")
    paths = {Path(__file__).resolve(), Path(sys.executable).resolve(), Path(sys.orig_argv[0]).resolve(),
             analytic.MANIFEST, analytic.REFERENCES}
    for module in tuple(sys.modules.values()):
        path = getattr(module, "__file__", None)
        if path and Path(path).is_file():
            paths.add(Path(path).resolve())
    result = {"schema": 1, "experiment": "start-current", "cells": matrix, "inputs": inputs,
              "producer_argv": list(sys.orig_argv),
              "source_files_sha256": {str(p): hash_regular_file(p) for p in sorted(paths)},
              "oracle_scope": "Frozen analytic construction, exact RGB over opaque white; not arbitrary PDF or continuous-scene truth"}
    (output / "inputs.json").write_bytes(encoded(result))


if __name__ == "__main__":
    main()
