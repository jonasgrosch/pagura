"""Read the independently authored, frozen analytic19 PDFs/oracles unchanged."""
import hashlib
import json
from pathlib import Path, PurePosixPath

from paired_render import ROOT, MAX_BYTES
from benchkit.artifacts import read_bounded_regular

MANIFEST = ROOT / "benchmarks/results/sources/analytic19-conversion/manifest.json"
REFERENCES = ROOT / "benchmarks/results/sources/analytic19-conversion/references.json"
MANIFEST_SHA256 = "278c79b1ccb29a2a5631050c7cd34f6e915312c9d98f59e0a600e2c94e3acac3"
REFERENCES_SHA256 = "924e20d6495990e8206c837d420c0172d5404ea1b3ede56417a9d59bb84bbcaf"
CASES = (
    "classic/blank", "classic/rectangle", "classic/graphics-state",
    "classic/negative-rectangle", "classic/inherited-box", "classic/clipped-to-page",
    "classic/many-rectangles", "classic/content-array-split", "classic/many-operands",
    "paint/text-heavy-page", "paint/text-mode-4", "paint/type3-d0-colour",
    "paint/text-knockout-true", "paint/text-knockout-false", "paint/image-soft-mask",
    "paint/image-direct-binary-halves", "paint/image-direct-gray-quarter-turn",
    "paint/image-direct-rgb-fractional", "paint/image-direct-bilinear",
)


def pinned(path, digest):
    raw = read_bounded_regular(path, MAX_BYTES)
    if hashlib.sha256(raw).hexdigest() != digest:
        raise ValueError("frozen analytic reference changed: " + str(path))
    return raw


def corpus_path(relative):
    pure = PurePosixPath(relative)
    if pure.is_absolute() or ".." in pure.parts or str(pure) != relative:
        raise ValueError("unsafe corpus reference")
    path = (ROOT / relative).resolve()
    if not path.is_relative_to(ROOT.resolve()):
        raise ValueError("corpus reference escapes checkout")
    return path


def fixture(name):
    if name not in CASES:
        raise ValueError("unknown frozen analytic case")
    manifest = json.loads(pinned(MANIFEST, MANIFEST_SHA256))
    references = json.loads(pinned(REFERENCES, REFERENCES_SHA256))
    documents, reference_cases = manifest["documents"], references["cases"]
    if (manifest["schema"] != 1 or references["schema"] != 1
            or tuple(row["id"] for row in documents) != CASES
            or tuple(row["id"] for row in reference_cases) != CASES
            or tuple(references["selector"]["case_ids"]) != CASES):
        raise ValueError("analytic19 selection/order drift")
    index = CASES.index(name)
    record, reference = documents[index], reference_cases[index]
    if record["pages"] != 1 or record["selected_pages"] != [0] or len(record["oracles"]) != 1:
        raise ValueError("expected one frozen first-page reference")
    oracle = record["oracles"][0]
    if (oracle["page"] != 0 or oracle["dpi"] != 72 or oracle["tolerance"] != 0
            or reference["expected"] != "render" or reference["dpi"] != 72
            or reference["path"] != record["path"] or reference["sha256"] != record["sha256"]
            or reference["oracle"] != oracle["path"] or reference["oracle_sha256"] != oracle["sha256"]):
        raise ValueError("reference policy/identity drift")
    return (pinned(corpus_path(record["path"]), record["sha256"]),
            pinned(corpus_path(oracle["path"]), oracle["sha256"]))
