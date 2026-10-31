"""Exact xref-delimited scalar integers, independently authored (7.2.3/7.3.10)."""
import zlib

from .document import Builder
from .model import OPEN, RENDER, Case
from .pdf import opaque_rectangles

AREA = "integer_separator"
REQUIREMENT = ("object-integer-separator",)
TAG = "object.integer-separator"
CONTENT = b"0 0 1 rg 0 0 32 32 re f 1 0 0 rg 4 4 8 8 re f 0 1 0 rg 20 20 4 4 re f"
ORACLE = opaque_rectangles(32, 32, [((0, 0, 32, 32), (0, 0, 255)), ((4, 4, 8, 8), (255, 0, 0)), ((20, 20, 4, 4), (0, 255, 0))])


def scalar(builder, number, value, suffix=b"\n", generation=0):
    builder.entries[number] = (1, len(builder.data), generation)
    builder.data += b"%d %d obj\n" % (number, generation) + value + b"endobj" + suffix


def page(value=None, *, separated=False, suffix=b"\n", flate=False, generation=0, reference_generation=0, chain=False, data_eol=True):
    builder = Builder()
    builder.obj(1, b"<< /Type /Catalog /Pages 2 0 R >>")
    builder.obj(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
    builder.obj(3, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << >> /Contents 4 0 R >>")
    data = zlib.compress(CONTENT) if flate else CONTENT
    # Standard controls use an EOL before endstream. Public-like malformed
    # twins retain the existing declared-boundary behavior separately.
    builder.obj(4, b"<< /Length 5 %d R " % reference_generation + (b"/Filter /FlateDecode " if flate else b"") + b">>\nstream\n" + data + (b"\n" if data_eol else b"") + b"endstream")
    raw = str(len(data)).encode() if value is None else value
    if chain:
        builder.obj(5, b"6 0 R")
        scalar(builder, 6, raw + (b" " if separated else b""), suffix)
    else:
        scalar(builder, 5, raw + (b" " if separated else b""), suffix, generation)
    return builder.classic()[0]


def updated(*, old_joined, current_joined):
    original = page(separated=not old_joined)
    previous = int(original.rsplit(b"startxref\n", 1)[1].splitlines()[0])
    builder = Builder()
    builder.data = bytearray(original)
    builder.entries = {}
    scalar(builder, 5, str(len(CONTENT)).encode() + (b"" if current_joined else b" "))
    return builder.classic(extra=b"/Prev %d " % previous)[0]


def cycle():
    original = page(chain=True)
    # Append a generation-correct reference update, with independent xref.
    previous = int(original.rsplit(b"startxref\n", 1)[1].splitlines()[0])
    builder = Builder()
    builder.data = bytearray(original)
    builder.entries = {}
    builder.obj(6, b"5 0 R")
    return builder.classic(extra=b"/Prev %d " % previous)[0]


def altered_xref(*, inside=False):
    original = page(data_eol=False)
    offset = original.index(b"5 0 obj\n")
    recorded = offset + (len(b"5 0 obj\n") + 1 if inside else 1)
    if not inside:
        return original.replace(b"%010d 00000 n" % offset, b"%010d 00000 n" % recorded)
    # An unrelated declared physical slot inside the scalar invalidates its
    # purported whole-object span. It must not be hidden by scalar recovery.
    previous = int(original.rsplit(b"startxref\n", 1)[1].splitlines()[0])
    builder = Builder()
    builder.data = bytearray(original)
    builder.entries = {6: (1, recorded, 0)}
    return builder.classic(extra=b"/Prev %d " % previous)[0]


def zero():
    builder = Builder()
    builder.obj(1, b"<< /Type /Catalog /Pages 2 0 R >>")
    builder.obj(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
    builder.obj(3, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << >> /Contents [4 0 R 6 0 R] >>")
    builder.obj(4, b"<< /Length 5 0 R >>\nstream\nendstream")
    scalar(builder, 5, b"0")
    builder.obj(6, b"<< /Length %d >>\nstream\n" % len(CONTENT) + CONTENT + b"\nendstream")
    return builder.classic()[0]


def cases():
    for name, options in [
        ("joined", {}), ("joined-flate", {"flate": True}),
        ("joined-source-like", {"data_eol": False}),
        ("joined-chain", {"chain": True}),
        ("joined-comment", {"suffix": b"% source comment containing 99endobj\n"}),
        ("joined-generation", {"generation": 2, "reference_generation": 2}),
        ("joined-leading-zero", {"value": b"000" + str(len(CONTENT)).encode()}),
    ]:
        yield Case(name, page(**options), RENDER, REQUIREMENT,
                   "Full blue/red/green page after exact scalar object repair", oracle=ORACLE,
                   strict_valid=False, repairs=(TAG,))
    for name, options in [("clean", {}), ("clean-flate", {"flate": True}), ("clean-chain", {"chain": True})]:
        yield Case(name, page(separated=True, **options), RENDER, REQUIREMENT,
                   "Legal separated scalar control", oracle=ORACLE)
    for name, value, options, error in [
        ("negative", b"-1", {}, "syntax"), ("positive-sign", b"+67", {}, "syntax"),
        ("real", b"67.0", {}, "syntax"), ("exponent", b"6.7e1", {}, "syntax"),
        ("reference", b"6 0 R", {}, "syntax"), ("array", b"[67]", {}, "syntax"),
        ("name", b"/67endobj", {}, "syntax"), ("string", b"(67endobj)", {}, "syntax"),
        ("hex-string", b"<3637656e646f626a>", {}, "syntax"),
        ("comment", b"% 67endobj", {}, "syntax"),
        ("overflow", b"9223372036854775808", {}, "syntax"),
        ("integer-max", b"9223372036854775807", {}, "limit"),
        ("trailing-token", None, {"suffix": b" null\n"}, "syntax"),
        ("generation-mismatch", None, {"generation": 1}, "syntax"),
    ]:
        repairs = (TAG,) if name == "integer-max" else ()
        yield Case(name, page(value, data_eol=False, **options), OPEN, REQUIREMENT,
                   "Invalid or unusable source cannot become a partial page",
                   strict_valid=not repairs, repairs=repairs, extra={"render_error": error})

    yield Case("incremental-current-joined", updated(old_joined=False, current_joined=True),
               RENDER, REQUIREMENT, "Newest exact xref slot supplies the joined integer", oracle=ORACLE,
               strict_valid=False, repairs=(TAG,))
    yield Case("incremental-old-joined-current-clean", updated(old_joined=True, current_joined=False),
               RENDER, REQUIREMENT, "A clean current integer does not normalize historical bytes", oracle=ORACLE)
    yield Case("length-cycle", cycle(), OPEN, REQUIREMENT,
               "A surviving indirect Length cycle is an explicit Cycle refusal",
               strict_valid=True, extra={"render_error": "cycle"})

    yield Case("joined-zero", zero(), RENDER, REQUIREMENT,
               "An exact zero-length stream preserves subsequent content", oracle=ORACLE,
               strict_valid=False, repairs=(TAG,))
    for name, pdf in [("corrected-offset", altered_xref()), ("overlapping-span", altered_xref(inside=True))]:
        yield Case(name, pdf, OPEN, REQUIREMENT,
                   "Nonexact or overlapping xref evidence cannot justify an integer repair",
                   strict_valid=name == "overlapping-span", extra={"render_error": "syntax", **({"strict_error_tag": "xref.offset-corrected"} if name == "corrected-offset" else {})})

    yield Case("unterminated-comment-boundary", page(data_eol=False, suffix=b"% no real line ending before "),
               OPEN, REQUIREMENT, "An EOF-truncated comment must not establish a physical boundary",
               strict_valid=True, extra={"render_error": "syntax"})

    # Budget checks use a source integer, not a platform-sized conversion.
    # These are declarations only; no large buffer is constructed.
    budget_requirement = ("stream-length-budget-portability",)
    cap = 256 * 1024 * 1024
    for name, declared, error in [
        ("declared-cap-minus1", cap-1, "syntax"),
        ("declared-cap-exact", cap, "syntax"),
        ("declared-cap-plus1", cap+1, "limit"),
        ("declared-u32-minus1", 2**32-2, "limit"),
        ("declared-u32-max", 2**32-1, "limit"),
        ("declared-u32-plus1", 2**32, "limit"),
        ("declared-i64-max", 2**63-1, "limit"),
        ("declared-negative", -1, "syntax"),
    ]:
        yield Case(name, page(str(declared).encode(), separated=True, data_eol=False),
                   OPEN, budget_requirement, "Portable declared Length refusal; no allocation or partial page",
                   strict_valid=True, extra={"render_error": error})
    for suffix, caller_cap in [("minus1", len(CONTENT)-1), ("exact", len(CONTENT)), ("plus1", len(CONTENT)+1)]:
        # Canonical conformance uses default limits and the full valid page.
        # The source-owned runtime probe also exercises this LOWER caller cap.
        extra: dict[str, object] = {"runtime_limits": {"stream_bytes": caller_cap}}
        if caller_cap < len(CONTENT):
            extra["runtime_expected_error_kind"] = "limit"
        yield Case("caller-cap-"+suffix, page(separated=True), RENDER, budget_requirement,
                   "Default full-page oracle plus explicit lower caller-cap probe", oracle=ORACLE, extra=extra)
