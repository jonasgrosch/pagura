"""Source-proven scalar reconstruction policy; ISO32000-2 7.5.4/7.5.5, ADR0003."""
from .model import INVALID, OPEN, RENDER, Case
from .pdf import opaque_rectangles

AREA = "original_records"
REQUIREMENT = ("original-record-scalar-reconstruction",)
INTEGER = "object.integer-separator"
REBUILT = "xref.rebuilt"
LENGTH = "stream.length-corrected"
CONTENT = b"1 0 0 rg 4 4 8 8 re f\n"
ORACLE = opaque_rectangles(32, 32, [((4, 4, 8, 8), (255, 0, 0))])


def page(*, joined=True, generation=0, scalar=5, size=2, omit_content=False,
         omit_scalar=False, offset_delta=0, record_generation=None, value=None,
         suffix=b"\n", trailer=b"", overlap=False, header=b"%PDF-1.7\n",
         leading=b"", relative=False, free_only=False):
    ids = [1, 2, 3, 4, scalar]
    raw = str(len(CONTENT)).encode() if value is None else value
    objects = {1: b"<< /Type /Catalog /Pages 2 0 R >>",
               2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
               3: b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << >> /Contents 4 0 R >>",
               4: b"<< /Length %d %d R >>\nstream\n" % (scalar, generation) + CONTENT + b"endstream",
               scalar: raw}
    data, offsets = leading + header, {}
    for number in ids:
        gen = generation if number == scalar else 0
        offsets[number] = len(data)
        data += b"%d %d obj\n" % (number, gen) + objects[number] + (b"" if number == scalar and joined else b"\n") + b"endobj" + (suffix if number == scalar else b"\n")
    xref = len(data)
    data += b"xref\n0 1\n0000000000 65535 f \n"
    if free_only:
        data += b"9 1\n0000000000 00000 f \n"
    else:
        for number in ids:
            if number == 4 and omit_content or number == scalar and omit_scalar:
                continue
            gen = (generation if record_generation is None else record_generation) if number == scalar else 0
            at = offsets[number] + (offset_delta if number == scalar else 0) - (len(leading) if relative else 0)
            data += b"%d 1\n%010d %05d n \n" % (number, at, gen)
        if overlap:
            data += b"%d 1\n%010d %05d n \n" % (scalar, offsets[scalar], generation)
    data += b"trailer\n<< /Root 1 0 R /Size %d" % size + trailer + b" >>\nstartxref\n%d\n%%%%EOF\n" % (xref - (len(leading) if relative else 0))
    return data, xref, offsets


def visitor(kind):
    objects = [(1, 0, b"<< /Type /Catalog /Pages 2 0 R >>", False),
               (2, 0, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>", False),
               (3, 0, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources << >> /Contents 4 0 R >>", False),
               (4, 0, b"<< /Length 5 0 R >>\nstream\n" + CONTENT + b"endstream", False),
               (5, 0, str(len(CONTENT)).encode(), kind in ["override", "generation-override"])]
    marker = b"7 0 obj\n22endobj\n"
    if kind == "stream":
        objects.append((6, 0, b"<< /Length %d >>\nstream\n" % len(marker) + marker + b"endstream", False))
    elif kind == "string":
        objects.append((6, 0, b"( visitor " + marker + b" )", False))
    elif kind == "comment":
        objects.append((6, 0, b"1 % visitor " + marker.replace(b"\n", b" ") + b"\n", False))
    elif kind == "name":
        objects.append((6, 0, b"/Visitor" + marker, False))
    data, offsets = b"%PDF-1.7\n", {}
    for number, generation, body, joined in objects:
        offsets[number] = len(data)
        data += b"%d %d obj\n" % (number, generation) + body + (b"" if joined else b"\n") + b"endobj\n"
    if kind in ["override", "generation-override"]:
        data += b"5 %d obj\n99\nendobj\n" % (1 if kind == "generation-override" else 0)
    else:
        offsets[7] = data.index(b"7 0 obj")
    xref = len(data)
    data += b"xref\n0 1\n0000000000 65535 f \n"
    for number, offset in offsets.items():
        data += b"%d 1\n%010d 00000 n \n" % (number, offset)
    data += b"trailer\n<< /Root 1 0 R /Size 2 >>\nstartxref\n%d\n%%%%EOF\n" % xref
    return data, xref


def strict_size(xref):
    # Strict returns the ORIGINAL untagged primary error, not xref.rebuilt.
    return {"strict": {"kind": "Syntax", "message": "trailer Size does not cover cross-reference entries",
                       "file_position": xref, "object": None, "repair": None}}


def cases():
    for name, options, query, eligible in [
        ("undersized-joined", {}, [5, 0], True),
        ("undersized-separated", {"joined": False}, [5, 0], False),
        ("positive-generation", {"generation": 7}, [5, 7], True),
        ("sparse-original", {"scalar": 1000, "size": 6}, [1000, 0], True),
        ("sparse-original-max", {"scalar": 2**32-1, "size": 6}, [2**32-1, 0], True),
        ("omitted-content-joined", {"omit_content": True}, [5, 0], True),
        ("omitted-content-separated", {"omit_content": True, "joined": False}, [5, 0], False),
        ("omitted-scalar-joined", {"omit_scalar": True}, [5, 0], False),
        ("wrong-scalar-offset", {"offset_delta": 1}, [5, 0], False),
        ("generation-mismatch", {"record_generation": 1}, [5, 0], False),
        ("duplicate-original-record", {"overlap": True}, [5, 0], False),
        ("negative-scalar", {"value": b"-21"}, [5, 0], False),
        ("real-scalar", {"value": b"21.0"}, [5, 0], False),
        ("reference-scalar", {"value": b"4 0 R"}, [5, 0], False),
        ("array-scalar", {"value": b"[21]"}, [5, 0], False),
        ("name-scalar", {"value": b"/21"}, [5, 0], False),
        ("string-scalar", {"value": b"(21)"}, [5, 0], False),
        ("integer-overflow", {"value": b"9223372036854775808"}, [5, 0], False),
        ("i64-max-scalar", {"value": b"9223372036854775807"}, [5, 0], True),
        ("comment-suffix-newline", {"suffix": b" % actual newline\n"}, [5, 0], True),
        ("comment-suffix-unterminated", {"suffix": b" % not terminated "}, [5, 0], False),
        ("prev-present", {"trailer": b" /Prev 0"}, [5, 0], False),
        ("xrefstm-present", {"trailer": b" /XRefStm 0"}, [5, 0], False),
    ]:
        data, xref, _ = page(**options)
        maximum = name == "i64-max-scalar"
        repairs = (REBUILT, INTEGER) if eligible else (REBUILT,) if name.endswith("separated") else (REBUILT, LENGTH)
        if name == "name-scalar":
            repairs += ("object.end-missing",)
        errors = strict_size(xref)
        if name == "duplicate-original-record":
            errors = {"strict": {"kind": "Syntax", "message": "subsections overlap", "repair": None,
                                 "file_position": data.rindex(b"n \ntrailer") + 1, "object": None}}
        if name == "xrefstm-present":
            errors = {"strict": {"kind": "Syntax", "message": "cross-reference stream Type must be XRef",
                                 "repair": None, "file_position": 0, "object": [1, 0]}}
        extra = {"mode_errors": errors, "original_scalar": query,
                 "original_record_authority": eligible}
        if maximum:
            extra["render_error"] = "limit"
        expected = "integer" if eligible or name.endswith("separated") else {"array-scalar": "array", "name-scalar": "name", "string-scalar": "string"}.get(name, "missing")
        value = (str(2**63-1) if maximum else 22) if expected == "integer" else 21 if expected == "array" else "21endobj" if expected == "name" else "21" if expected == "string" else None
        extra["document_queries"] = [{"object": query, "expected": expected, "value": value}]
        if name == "positive-generation":
            extra["document_queries"].append({"object": [5, 0], "expected": "missing"})
        yield Case(name, data, OPEN if maximum else RENDER, REQUIREMENT,
                   "Existing scanner visitation and exact original records; declined proofs retain prior bounded stream recovery",
                   oracle=None if maximum else ORACLE, strict_valid=False, repairs=repairs, extra=extra)
    for name, joined in [("size-valid-joined", True), ("size-valid-separated", False)]:
        data, _, _ = page(size=6, joined=joined)
        yield Case(name, data, RENDER, REQUIREMENT, "Unchanged primary-index scalar control", oracle=ORACLE,
                   strict_valid=not joined, repairs=(INTEGER,) if joined else (), extra={"original_scalar": [5, 0], "document_queries": [{"object": [5, 0], "expected": "integer", "value": 22}]})
    for name, kind in [("joined-then-clean-override", "override"), ("joined-then-generation-override", "generation-override"),
                       ("fake-header-in-stream", "stream"), ("fake-header-in-string", "string"),
                       ("fake-header-in-comment", "comment"), ("fake-header-in-name", "name")]:
        data, xref = visitor(kind)
        override = kind in ["override", "generation-override"]
        repairs = (REBUILT, INTEGER) if override else (REBUILT,)
        if override:
            repairs += (LENGTH,)
        queries = ([{"object": [5, 0], "expected": "integer", "value": 99}] if kind == "override" else
                   [{"object": [5, 1], "expected": "integer", "value": 99}, {"object": [5, 0], "expected": "missing"}] if kind == "generation-override" else
                   [{"object": [7, 0], "expected": "missing"}])
        errors = strict_size(xref)
        boundary_refusal = kind == "override"
        if boundary_refusal:
            errors["repair"] = {"kind": "Syntax", "message": "stream Length crosses an object boundary",
                                "file_position": data.index(b"stream\n") + len(b"stream\n") + 99,
                                "object": [4, 0], "repair": None}
        extra = {"mode_errors": errors, "document_queries": queries}
        if boundary_refusal:
            extra["render_error"] = "syntax"
        yield Case(name, data, OPEN if boundary_refusal else RENDER, REQUIREMENT,
                   "Fake headers remain unvisited; final definitions retain existing stream boundary refusals",
                   oracle=None if boundary_refusal else ORACLE,
                   strict_valid=False, repairs=(REBUILT, INTEGER) if boundary_refusal else repairs, extra=extra)
    for name, options, tag in [
        ("encrypt-null", {"trailer": b" /Encrypt null"}, None),
        ("encrypt-undefined-reference", {"trailer": b" /Encrypt 99 0 R"}, None),
        ("header-extra-format", {"header": b"%PDF-1.7 extra\n"}, "file.header-format"),
        ("leading-junk-physical-offsets", {"leading": b"junk\n"}, "file.header-offset"),
        ("leading-junk-relative-offsets", {"leading": b"junk\n", "relative": True}, "file.header-offset"),
        ("missing-pdf-header", {"header": b"%XYZ-1.7\n"}, None),
        ("all-free-size-coverage", {"free_only": True}, None),
    ]:
        data, xref, _ = page(**options)
        invalid = name in ["encrypt-undefined-reference", "missing-pdf-header"]
        errors = strict_size(xref)
        if tag:
            errors = {"strict": {"kind": "Syntax", "repair": tag,
                                 "file_position": 8 if tag == "file.header-format" else 5, "object": None}}
        if name == "missing-pdf-header":
            errors = {mode: {"kind": "Syntax", "message": "missing or unknown PDF header", "repair": None,
                             "file_position": 0, "object": None} for mode in ["strict", "repair"]}
        if name == "encrypt-undefined-reference":
            errors["repair"] = {"kind": "Syntax", "message": "expected a dictionary", "repair": None, "object": [99, 0]}
        yield Case(name, data, INVALID if invalid else RENDER, REQUIREMENT,
                   "Pristine header, absent Encrypt and useful physical-record scope; no new original authority",
                   oracle=None if invalid else ORACLE, pages=None if invalid else 1, strict_valid=False,
                   repairs=() if invalid else ((tag,) if tag else ()) + (REBUILT, LENGTH),
                   extra={"mode_errors": errors, "render_error": "syntax"} if invalid else {"mode_errors": errors, "document_queries": [{"object": [5, 0], "expected": "missing"}]})
