"""Pinned, independent-reference RGB8 comparison; not release admission evidence.

This verifies bytes and a deliberately narrow metadata contract. Independence,
the mathematical derivation, and completeness of the render policy need review.
The caller must obtain the metadata pin outside the candidate report and bind
the supplied settings to the actual render invocation. A returned dictionary is
not an attestation of execution and must not replace release-gate pixel hashes.
"""
import hashlib
import json
import re

from .images import ppm, verify_pixels


def _hash(data):
    return hashlib.sha256(data).hexdigest()


def _keys(value, expected, label):
    if type(value) is not dict or set(value) != set(expected):
        raise ValueError(f"invalid {label} fields")


def _digest(value, label):
    if type(value) is not str or re.fullmatch(r"[0-9a-f]{64}", value) is None:
        raise ValueError(f"invalid {label} digest")


def _path(value):
    if (type(value) is not str or not value or "\\" in value or "\0" in value
            or any(part in ("", ".", "..") for part in value.split("/"))):
        raise ValueError("sources require canonical relative POSIX paths")


def _settings(value):
    _keys(value, ("source_pdf_sha256", "render_policy_sha256", "page", "dpi",
                  "background", "output", "box", "annotations", "accuracy",
                  "entrypoint", "validation"), "settings")
    for key in ("source_pdf_sha256", "render_policy_sha256"):
        _digest(value[key], key)
    if type(value["page"]) is not int or value["page"] < 0:
        raise ValueError("page must be a nonnegative integer")
    if type(value["dpi"]) is not int or not 1 <= value["dpi"] <= 9600:
        raise ValueError("dpi must be an integer in 1..9600")
    if (value["background"] != "white" or value["output"] != "RGB8 PPM"
            or value["box"] != "MediaBox=CropBox" or value["annotations"] is not True):
        raise ValueError("unsupported RGB8 rendering policy")
    if value["accuracy"] not in ("exact", "competitive"):
        raise ValueError("unsupported accuracy")
    if value["entrypoint"] not in ("render_page", "compile_page+render"):
        raise ValueError("unsupported entrypoint")
    if value["validation"] not in ("full", "drawing-only"):
        raise ValueError("unsupported validation scope")
    if (value["validation"] == "drawing-only"
            and (value["accuracy"] != "competitive" or value["entrypoint"] != "render_page")):
        raise ValueError("drawing-only validation requires competitive render_page")


def reference_digest(metadata):
    """Validate v1 metadata and hash its canonical UTF-8 JSON representation.

    render_policy_sha256 identifies a reviewed policy document covering fonts,
    color, layers, recovery, rotation/UserUnit, filtering, and coverage semantics.
    It must be included among the supplied sources, like the source PDF. This
    function cannot establish that either policy or derivation is correct.
    """
    _keys(metadata, ("schema", "case_id", "settings", "dimensions", "reference",
                     "oracle_sha256", "sources", "max_channel_error"), "reference metadata")
    if type(metadata["schema"]) is not int or metadata["schema"] != 1:
        raise ValueError("unsupported reference schema")
    if type(metadata["case_id"]) is not str or not metadata["case_id"].strip():
        raise ValueError("missing case identity")
    _settings(metadata["settings"])
    dimensions = metadata["dimensions"]
    if (type(dimensions) is not list or len(dimensions) != 2
            or any(type(value) is not int or value <= 0 for value in dimensions)
            or dimensions[0] * dimensions[1] > 16_000_000):
        raise ValueError("invalid reference dimensions")
    reference = metadata["reference"]
    _keys(reference, ("model", "evidence_class", "rounding", "derivation"), "reference")
    if reference["model"] not in ("decoded-samples-rgb8", "polygon-coverage-rgb8",
                                   "per-mark-coverage-rgb8"):
        raise ValueError("unsupported reference model")
    if reference["evidence_class"] != "exact_rational":
        raise ValueError("v1 requires a reviewed exact_rational derivation")
    if reference["rounding"] != "nearest-half-up":
        raise ValueError("unsupported reference rounding")
    if type(metadata["max_channel_error"]) is not int or metadata["max_channel_error"] not in (0, 1):
        raise ValueError("final RGB8 bound must be zero or one")
    _digest(metadata["oracle_sha256"], "oracle")
    sources = metadata["sources"]
    if type(sources) is not dict or not sources:
        raise ValueError("missing reference sources")
    for path, digest in sources.items():
        _path(path)
        _digest(digest, "source")
    _path(reference["derivation"])
    if reference["derivation"] not in sources:
        raise ValueError("derivation must identify a bound source")
    for key in ("source_pdf_sha256", "render_policy_sha256"):
        if metadata["settings"][key] not in sources.values():
            raise ValueError(f"{key} must identify a bound source")
    return _hash(json.dumps(metadata, sort_keys=True, separators=(",", ":"),
                            ensure_ascii=True, allow_nan=False).encode("utf-8"))


def verify_reference(actual_ppm, oracle_ppm, metadata, *, pinned_digest, settings, sources):
    """Compare real RGB8 bytes against frozen reference data and its final bound.

    Invalid/missing provenance or malformed images raise ValueError. Valid RGB8
    output outside the bound (including a dimension mismatch) is incorrect.
    Only exact-rational metadata is supported; numerical estimates or interval
    proofs cannot be verified by a single expected pixel array in this contract.
    """
    _digest(pinned_digest, "pinned reference")
    digest = reference_digest(metadata)
    if digest != pinned_digest:
        raise ValueError("reference metadata differs from the frozen pin")
    _settings(settings)
    if settings != metadata["settings"]:
        raise ValueError("actual settings differ from reference settings")
    if type(sources) is not dict or set(sources) != set(metadata["sources"]):
        raise ValueError("missing or unexpected reference sources")
    for path, data in sources.items():
        if type(data) is not bytes or _hash(data) != metadata["sources"][path]:
            raise ValueError(f"reference source hash mismatch: {path}")
    if type(actual_ppm) is not bytes or type(oracle_ppm) is not bytes:
        raise ValueError("images must be immutable bytes")
    if _hash(oracle_ppm) != metadata["oracle_sha256"]:
        raise ValueError("oracle artifact hash mismatch")
    actual, expected = ppm(actual_ppm), ppm(oracle_ppm)
    if list(expected[:2]) != metadata["dimensions"]:
        raise ValueError("oracle dimensions differ from reference metadata")
    return {
        **verify_pixels(actual, expected, metadata["max_channel_error"]),
        "case_id": metadata["case_id"], "reference_digest": digest,
        "reference_model": metadata["reference"]["model"],
        "evidence_class": metadata["reference"]["evidence_class"],
        "actual_sha256": _hash(actual_ppm), "actual_pixels_sha256": _hash(actual[2]),
        "oracle_sha256": _hash(oracle_ppm), "oracle_pixels_sha256": _hash(expected[2]),
        "actual_dimensions": list(actual[:2]), "reference_dimensions": list(expected[:2]),
    }
