"""Bind reviewed fixture references to source bytes and explicit invocations.

Only the four image-direct fixtures have reviewed v1 profiles. This module
does not infer independence from arbitrary manifest claims, attest execution,
or relax the benchmark release gate. Manifest records must come from the
frozen, reviewed fixture configuration, outside candidate reports.
"""
import hashlib
import os
import re
import stat
from pathlib import Path

from .images import ppm
from .oracles import reference_digest, verify_reference

PROFILE = "image-direct-v1"
POLICY_PATH = "docs/references/image-direct-policy.md"
DERIVATION_PATH = "docs/references/image-direct-derivation.md"
CASE_IDS = frozenset("paint/image-direct-" + suffix for suffix in (
    "binary-halves", "gray-quarter-turn", "rgb-fractional", "bilinear"))
MODES = ("exact", "competitive")
# Reviewed repository-local sources for this family, including the helper
# imports and PDF writer. Python's standard library is not a hermetic runtime.
COMMON_SOURCES = (
    POLICY_PATH, DERIVATION_PATH,
    "tools/fixtures.py", "tools/fixturegen/__init__.py",
    "tools/fixturegen/model.py", "tools/fixturegen/paint.py",
    "tools/fixturegen/pdf.py", "tools/fixturegen/image.py",
    "tools/fixturegen/font.py", "tools/fixturegen/raster.py",
    "tools/fixturegen/jpx.py", "tools/fixturegen/jbig2.py",
    "tools/fixturegen/competitive.py", "tools/benchkit/__init__.py",
    "tools/benchkit/reference_cases.py", "tools/benchkit/oracles.py",
    "tools/benchkit/images.py",
)
MAX_SOURCE_FILES = 32
MAX_SOURCE_BYTES = 4 * 1024 * 1024
MAX_TOTAL_SOURCE_BYTES = 16 * 1024 * 1024


def sha(data):
    return hashlib.sha256(data).hexdigest()


def requires_reference(case):
    """Known fixtures cannot silently lose their annotated verification path."""
    if type(case) is not dict:
        raise ValueError("reference case must be an object")
    return ((type(case.get("id")) is str and case["id"] in CASE_IDS) or "reference_profile" in case
            or "render_references" in case)


def invocation_settings(source_pdf_sha256, *, page, dpi, accuracy,
                        entrypoint, validation, render_policy_sha256,
                        profile=PROFILE):
    """Describe a caller's known invocation, never by copying reference settings.

    The caller selects the reviewed profile and supplies the hash of its policy
    bytes. Source loading separately checks that policy against the frozen pin.
    This profile has no other page, resolution, entrypoint or output contract.
    """
    if profile != PROFILE:
        raise ValueError("unsupported reference profile")
    if (type(page) is not int or page != 0 or type(dpi) is not int or dpi != 72
            or accuracy not in MODES or entrypoint != "render_page"
            or validation != ("full" if accuracy == "exact" else "drawing-only")):
        raise ValueError("unsupported reference invocation")
    for digest in (source_pdf_sha256, render_policy_sha256):
        if type(digest) is not str or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
            raise ValueError("invalid invocation digest")
    return {
        "source_pdf_sha256": source_pdf_sha256,
        "render_policy_sha256": render_policy_sha256,
        "page": page, "dpi": dpi, "background": "white", "output": "RGB8 PPM",
        "box": "MediaBox=CropBox", "annotations": True, "accuracy": accuracy,
        "entrypoint": entrypoint, "validation": validation,
    }


def read_reference_file(root, relative, *, limit=MAX_SOURCE_BYTES):
    """Read a bounded regular file whose resolved path remains within root."""
    if (type(relative) is not str or not relative or "\\" in relative or "\0" in relative
            or any(part in ("", ".", "..") for part in relative.split("/"))):
        raise ValueError("reference paths must be canonical relative POSIX paths")
    base = Path(root).resolve(strict=True)
    try:
        path = (base / relative).resolve(strict=True)
        if not path.is_relative_to(base) or not path.is_file():
            raise ValueError("reference source escapes root or is not a regular file")
        descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as stream:
            if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
                raise ValueError("reference source is not a regular file")
            data = stream.read(limit + 1)
    except OSError as error:
        raise ValueError(f"cannot read reference source: {relative}") from error
    if len(data) > limit:
        raise ValueError("reference source exceeds byte budget")
    return data


def load_reference_sources(root, metadata):
    """Read and rehash every named source, with per-file and aggregate bounds."""
    reference_digest(metadata)
    if len(metadata["sources"]) > MAX_SOURCE_FILES:
        raise ValueError("reference source count exceeds budget")
    sources, total = {}, 0
    for relative, digest in metadata["sources"].items():
        data = read_reference_file(root, relative, limit=min(
            MAX_SOURCE_BYTES, MAX_TOTAL_SOURCE_BYTES - total))
        if sha(data) != digest:
            raise ValueError(f"reference source hash mismatch: {relative}")
        sources[relative] = data
        total += len(data)
    return sources


def current_render_policy_sha256(root):
    """Hash the caller-selected, supported policy file, not candidate metadata."""
    return sha(read_reference_file(root, POLICY_PATH))


def _profile_case(case):
    if (type(case) is not dict or case.get("reference_profile") != PROFILE
            or type(case.get("id")) is not str or case["id"] not in CASE_IDS or case.get("kind") != "pdf"
            or case.get("expected") != "render"):
        raise ValueError("unsupported annotated reference case")
    for key, expected in (("dpi", 72), ("width", 32), ("height", 32)):
        if type(case.get(key)) is not int or case[key] != expected:
            raise ValueError(f"reference case has unsupported {key}")
    for key, expected in (("page", 0), ("pages", 1), ("page_count", 1)):
        if key in case and (type(case[key]) is not int or case[key] != expected):
            raise ValueError(f"reference case has unsupported {key}")
    if (case.get("strict_valid") is not True or case.get("repairs") != []
            or case.get("information", []) != [] or "password" in case
            or type(case.get("path")) is not str or type(case.get("oracle")) is not str):
        raise ValueError("reference case has unsupported source or diagnostic policy")
    for mode in MODES:
        bound = case.get("tolerance", 0)
        if mode == "competitive":
            bound = case.get("competitive_tolerance", bound)
        expected = int(mode == "competitive" and case["id"] == "paint/image-direct-rgb-fractional")
        if type(bound) is not int or bound != expected:
            raise ValueError("reference profile cannot change the existing final tolerance")
    return f"corpus/generated/{case['id']}.pdf"


def _reference_model():
    return {"model": "decoded-samples-rgb8", "evidence_class": "exact_rational",
            "rounding": "nearest-half-up", "derivation": DERIVATION_PATH}


def build_case_references(root, case, *, pdf_bytes, oracle_bytes):
    """Generate pins after Case.extra and existing Competitive bounds are applied.

    Regeneration is not approval: the manifest and these sources require review.
    No product output is read to construct or adjust the reference.
    """
    pdf_path = _profile_case(case)
    if "render_references" in case:
        raise ValueError("reference records must be generated after case assembly")
    if case["path"] != pdf_path or case["oracle"] != pdf_path[:-4] + ".ppm":
        raise ValueError("assembled reference paths differ from generated paths")
    if type(pdf_bytes) is not bytes or sha(pdf_bytes) != case.get("sha256"):
        raise ValueError("assembled PDF identity differs from generated bytes")
    if type(oracle_bytes) is not bytes or sha(oracle_bytes) != case.get("oracle_sha256"):
        raise ValueError("assembled oracle identity differs from generated bytes")
    if ppm(oracle_bytes)[:2] != (case["width"], case["height"]):
        raise ValueError("assembled oracle dimensions differ from generated bytes")
    sources = {path: sha(read_reference_file(root, path)) for path in COMMON_SOURCES}
    sources[pdf_path] = sha(pdf_bytes)
    records = {}
    for mode in MODES:
        metadata = {
            "schema": 1, "case_id": case["id"],
            "settings": invocation_settings(sha(pdf_bytes), page=0, dpi=case["dpi"],
                accuracy=mode, entrypoint="render_page",
                validation="full" if mode == "exact" else "drawing-only",
                render_policy_sha256=sources[POLICY_PATH]),
            "dimensions": [case["width"], case["height"]],
            "reference": _reference_model(), "oracle_sha256": sha(oracle_bytes),
            "sources": sources.copy(),
            "max_channel_error": (case.get("competitive_tolerance", case.get("tolerance", 0))
                                  if mode == "competitive" else case.get("tolerance", 0)),
        }
        records[mode] = {"metadata": metadata, "sha256": reference_digest(metadata)}
    return records


def validate_case_reference(case, mode):
    """Resolve one frozen pin, rejecting conflicting final case fields or modes."""
    pdf_path = _profile_case(case)
    if mode not in MODES:
        raise ValueError("unsupported reference mode")
    records = case.get("render_references")
    if type(records) is not dict or set(records) != set(MODES):
        raise ValueError("reference cases require both mode records")
    for accuracy, record in records.items():
        if type(record) is not dict or set(record) != {"metadata", "sha256"}:
            raise ValueError("invalid case reference record")
        metadata = record["metadata"]
        if reference_digest(metadata) != record["sha256"]:
            raise ValueError("case reference differs from its frozen pin")
        expected_sources = set(COMMON_SOURCES) | {pdf_path}
        if (set(metadata["sources"]) != expected_sources
                or metadata["sources"][pdf_path] != case.get("sha256")):
            raise ValueError("reference case source set or PDF identity differs")
        expected_settings = invocation_settings(case.get("sha256"), page=0, dpi=case["dpi"],
            accuracy=accuracy, entrypoint="render_page",
            validation="full" if accuracy == "exact" else "drawing-only",
            render_policy_sha256=metadata["sources"][POLICY_PATH])
        bound = (case.get("competitive_tolerance", case.get("tolerance", 0))
                 if accuracy == "competitive" else case.get("tolerance", 0))
        if (metadata["case_id"] != case["id"] or metadata["settings"] != expected_settings
                or metadata["dimensions"] != [case["width"], case["height"]]
                or metadata["oracle_sha256"] != case.get("oracle_sha256")
                or metadata["reference"] != _reference_model()
                or metadata["max_channel_error"] != bound):
            raise ValueError("reference metadata differs from the assembled case")
    if records["exact"]["metadata"]["sources"] != records["competitive"]["metadata"]["sources"]:
        raise ValueError("mode records disagree on the reference source set")
    record = records[mode]
    return record["metadata"], record["sha256"]


def verify_case_reference(root, case, mode, actual_ppm, *, settings, source_bytes=None):
    """Compare bytes using frozen case records and independently supplied settings.

    source_bytes, when supplied, is the actual immutable PDF input snapshot.
    The helper still revalidates all frozen repository sources. Execution and
    artifact receipts belong to the caller; a comparison is not an attestation.
    """
    metadata, pin = validate_case_reference(case, mode)
    sources = load_reference_sources(root, metadata)
    pdf_path = f"corpus/generated/{case['id']}.pdf"
    if source_bytes is not None and (type(source_bytes) is not bytes
            or sha(source_bytes) != metadata["sources"][pdf_path]):
        raise ValueError("actual PDF snapshot differs from the reference source")
    oracle = read_reference_file(root, f"corpus/generated/{case['id']}.ppm")
    return verify_reference(actual_ppm, oracle, metadata, pinned_digest=pin,
                            settings=settings, sources=sources)
