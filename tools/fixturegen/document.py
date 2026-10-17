"""Independent document-structure oracles, ISO 32000 clauses 7.5–7.6.

Only Python's standard library is used. Offsets, compressed-object indexes,
revision precedence and pixels are computed while constructing each file;
Pagura is never invoked to derive an expectation.
"""
from __future__ import annotations

import hashlib
import struct
import zlib

from .model import INVALID, OPEN, RENDER, Case
from .pdf import HEADER, document, opaque_rectangles, stream

AREA = "document"
CONTENT = b"1 0 0 rg 8 8 48 32 re f\n"
OBJECTS = [
    b"<< /Type /Catalog /Pages 2 0 R >>",
    b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 64 64] /Resources << >> /Contents 4 0 R >>",
    stream(CONTENT),
]
RED = opaque_rectangles(64, 64, [((8, 8, 48, 32), (255, 0, 0))])


class Builder:
    def __init__(self):
        self.data = bytearray(HEADER)
        self.entries = {0: (0, 0, 65535)}

    def obj(self, number, body, generation=0):
        self.entries[number] = (1, len(self.data), generation)
        self.data += b"%d %d obj\n" % (number, generation) + body + b"\nendobj\n"

    def packed(self, number=5, values=None, extra=b"", flate=False):
        if values is None:
            values = [(i, body) for i, body in enumerate(OBJECTS[:3], 1)]
        data, pairs = bytearray(), []
        for index, (key, body) in enumerate(values):
            pairs.append(b"%d %d" % (key, len(data)))
            self.entries[key] = (2, number, index)
            data += body + b"\n"
        header = b" ".join(pairs) + b" "
        raw = header + data
        params = b"/Type /ObjStm /N %d /First %d " % (len(values), len(header)) + extra
        if flate:
            raw = zlib.compress(raw)
            params += b" /Filter /FlateDecode "
        self.obj(number, stream(raw, params))

    def xref(self, number=6, widths=(1, 4, 2), index=None, extra=b"", flate=False, predictor=False):
        offset = len(self.data)
        self.entries[number] = (1, offset, 0)
        size = max(self.entries) + 1
        index = index if index is not None else [(0, size)]
        rows = []
        for first, count in index:
            for key in range(first, first + count):
                entry = self.entries.get(key, (0, 0, 0))
                rows.append(b"".join(value.to_bytes(width, "big") for value, width in zip(entry, widths) if width))
        raw = b"".join(rows)
        params = b"/Type /XRef /Root 1 0 R /Size %d /W [%s] /Index [%s] " % (
            size, b" ".join(str(w).encode() for w in widths),
            b" ".join(b"%d %d" % pair for pair in index)) + extra
        if predictor:
            previous = bytes(sum(widths))
            predicted = bytearray()
            for row in rows:
                predicted += b"\x02" + bytes((value - before) % 256 for value, before in zip(row, previous))
                previous = row
            raw = bytes(predicted)
            params += b" /DecodeParms << /Predictor 12 /Columns %d >> " % sum(widths)
        if flate:
            raw = zlib.compress(raw)
            params += b" /Filter /FlateDecode "
        self.obj(number, stream(raw, params))
        self.data += b"startxref\n%d\n%%%%EOF\n" % offset
        return bytes(self.data)

    def classic(self, extra=b"", entries=None, root=True):
        offset = len(self.data)
        self.data += b"xref\n"
        for key, (kind, second, third) in sorted((entries or self.entries).items()):
            if kind == 2:
                kind, second, third = 0, 0, 0
            self.data += b"%d 1\n%010d %05d %s \n" % (key, second, third, b"n" if kind else b"f")
        self.data += b"trailer\n<< /Size %d %s %s>>\nstartxref\n%d\n%%%%EOF\n" % (
            max(self.entries) + 1, b"/Root 1 0 R" if root else b"", extra, offset)
        return bytes(self.data), offset


def base():
    builder = Builder()
    for number, body in enumerate(OBJECTS, 1):
        builder.obj(number, body)
    return builder


def rendered(name, pdf, description, requirement="xref-streams", **kwargs):
    return Case(name, pdf, RENDER, (requirement,), description, oracle=RED, **kwargs)


# ISO 32000-1/2 algorithms 2–5, implemented independently for fixture creation.
PADDING = bytes.fromhex("28bf4e5e4e758a4164004e56fffa01082e2e00b6d0 683e802f0ca9fe6453697a")


def rc4(key, data):
    state = list(range(256))
    j = 0
    for i in range(256):
        j = (j + state[i] + key[i % len(key)]) % 256
        state[i], state[j] = state[j], state[i]
    i = j = 0
    output = bytearray()
    for value in data:
        i = (i + 1) % 256
        j = (j + state[i]) % 256
        state[i], state[j] = state[j], state[i]
        output.append(value ^ state[(state[i] + state[j]) % 256])
    return bytes(output)


def encrypted(revision, identity=False, compressed=False):
    assert len(PADDING) == 32
    assert rc4(b"Key", b"Plaintext").hex() == "bbf316e8d940af0ad3"
    version = {2: 1, 3: 2, 4: 4}[revision]
    length = 5 if revision == 2 else 16
    digest = hashlib.md5((b"owner" + PADDING)[:32]).digest()
    if revision >= 3:
        for _ in range(50):
            digest = hashlib.md5(digest).digest()
    owner_key = digest[:length]
    owner = rc4(owner_key, PADDING)
    if revision >= 3:
        for iteration in range(1, 20):
            owner = rc4(bytes(value ^ iteration for value in owner_key), owner)
    file_id = bytes.fromhex("00112233445566778899aabbccddeeff")
    digest = hashlib.md5(PADDING + owner + struct.pack("<i", -4) + file_id).digest()
    if revision >= 3:
        for _ in range(50):
            digest = hashlib.md5(digest[:length]).digest()
    key = digest[:length]
    user = rc4(key, PADDING if revision == 2 else hashlib.md5(PADDING + file_id).digest())
    if revision >= 3:
        for iteration in range(1, 20):
            user = rc4(bytes(value ^ iteration for value in key), user)
        user += bytes(16)

    def encrypt(number, data):
        object_key = hashlib.md5(key + number.to_bytes(3, "little") + bytes(2)).digest()[:min(length + 5, 16)]
        return rc4(object_key, data)

    encryption = b"<< /Filter /Standard /V %d /R %d /Length %d /O <%s> /U <%s> /P -4 " % (
        version, revision, length * 8, owner.hex().encode(), user.hex().encode())
    if revision == 4:
        encryption += b"/CF << /RC << /CFM /V2 /Length 16 >> >> /StrF /RC /StmF /RC "
    encryption += b">>"
    objects = list(OBJECTS)
    objects[0] = OBJECTS[0].replace(b"/Pages", b"/Marker <" + encrypt(1, b"independent").hex().encode() + b"> /Pages")
    objects[3] = stream(CONTENT if identity else encrypt(4, CONTENT),
                        b"/Filter /Crypt /DecodeParms << /Name /Identity >> " if identity else b"")
    objects.append(encryption)
    identifier = b"/ID [<" + file_id.hex().encode() + b"><" + file_id.hex().encode() + b">] "
    if compressed:
        builder = Builder()
        builder.obj(4, objects[3])
        data, pairs = bytearray(), []
        values = list(OBJECTS[:3])
        values[0] = values[0].replace(b"/Pages", b"/Marker (independent) /Pages")
        for index, body in enumerate(values):
            pairs.append(b"%d %d" % (index + 1, len(data)))
            builder.entries[index + 1] = (2, 5, index)
            data += body + b"\n"
        header = b" ".join(pairs) + b" "
        builder.obj(5, stream(encrypt(5, header + data), b"/Type /ObjStm /N 3 /First %d " % len(header)))
        builder.obj(7, encryption)
        return builder.xref(extra=b"/Encrypt 7 0 R " + identifier)
    return document(objects, trailer_extra=b"/Encrypt 5 0 R " + identifier)


def cases():
    yield rendered("xref-stream", base().xref(), "Unfiltered type 0 and 1 cross-reference stream")
    yield rendered("xref-index", base().xref(index=[(0, 2), (2, 2), (4, 3)]),
                   "Several Index subsections cover disjoint object ranges")
    yield rendered("xref-default-fields", base().xref(widths=(0, 4, 0), index=[(1, 6)]),
                   "Zero-width type and generation fields default to 1 and 0")
    builder = base()
    builder.entries[7] = (99, 0, 0)
    yield rendered("xref-unknown-type", builder.xref(), "Unknown entry types represent null references")
    for predictor in [False, True]:
        yield rendered("xref-flate-predictor" if predictor else "xref-flate",
                       base().xref(flate=True, predictor=predictor),
                       "Flate cross-reference stream, optionally using independent PNG Up prediction",
                       extra={})
    builder = Builder()
    builder.obj(4, OBJECTS[3])
    builder.packed(extra=b"/Extends 7 0 R ")
    builder.obj(7, stream(b"", b"/Type /ObjStm /N 0 /First 0 "))
    yield rendered("object-stream", builder.xref(number=6),
                   "Catalog, Pages and Page in one object stream; Extends describes a collection",
                   requirement="object-streams")
    builder = Builder()
    builder.obj(4, OBJECTS[3])
    builder.packed(flate=True)
    yield rendered("object-stream-flate", builder.xref(), "Flate object stream",
                   requirement="object-streams", extra={})
    builder = Builder()
    builder.obj(4, OBJECTS[3])
    builder.packed(values=[(1, OBJECTS[0]), (2, OBJECTS[1]),
                           (3, OBJECTS[2].replace(b"<< >>", b"7 0 R")),
                           (7, b"8 0 R"), (8, b"<< /Marker (independent) >>")])
    yield rendered("compressed-reference-chain", builder.xref(number=9),
                   "References between compressed objects resolve within the same decoded stream",
                   requirement="object-streams")
    builder = Builder()
    builder.obj(4, OBJECTS[3])
    builder.packed()
    supplemental_offset = len(builder.data)
    builder.xref()
    hybrid, _ = builder.classic(extra=b"/XRefStm %d " % supplemental_offset)
    yield rendered("hybrid-precedence", hybrid, "Supplemental compressed entries override free table entries",
                   requirement="document-revisions")
    builder = base()
    _, previous = builder.classic()
    builder.obj(4, stream(CONTENT))
    updated, _ = builder.classic(extra=b"/Prev %d " % previous, entries={4: builder.entries[4]}, root=False)
    yield rendered("revision-redefine", updated, "Newest entry wins and missing Root inherits from the older trailer",
                   requirement="document-revisions")
    builder = base()
    _, previous = builder.classic()
    builder.obj(2, b"<< /Type /Pages /Kids [] /Count 0 >>")
    builder.entries[3] = (0, 0, 1)
    updated, _ = builder.classic(extra=b"/Prev %d " % previous, entries={2: builder.entries[2], 3: builder.entries[3]})
    yield Case("revision-free", updated, OPEN, ("document-revisions",),
               "Free entry deletes the old page; revised page tree has zero leaves", pages=0)
    builder = base()
    at = len(builder.data)
    cycle, _ = builder.classic(extra=b"/Prev %d " % at)
    yield rendered("prev-cycle", cycle, "Strict Prev cycle error; repair reconstructs complete objects",
                   requirement="document-revisions", strict_valid=False, repairs=("xref.rebuilt",))
    builder = base()
    at = len(builder.data)
    yield rendered("stream-prev-cycle", builder.xref(extra=b"/Prev %d " % at),
                   "Strict stream Prev cycle error; repair reconstructs complete objects",
                   requirement="document-revisions", strict_valid=False, repairs=("xref.rebuilt",))
    builder = base()
    kind, offset, generation = builder.entries[1]
    builder.entries[1] = (kind, offset + 2, generation)
    damaged, _ = builder.classic()
    yield rendered("nearby-offset", damaged, "Nearby exact matching Catalog header repairs its wrong offset",
                   requirement="document-repairs", strict_valid=False, repairs=("xref.offset-corrected",))
    yield rendered("header-offset", b"junk\n" + document(OBJECTS), "Prepended junk shifts recorded offsets",
                   requirement="document-repairs", strict_valid=False, repairs=("file.header-offset",))
    builder = base()
    builder.data = bytearray(b"junk\n" + HEADER)
    builder.entries = {0: (0, 0, 65535)}
    for number, body in enumerate(OBJECTS, 1):
        builder.obj(number, body)
    physical, _ = builder.classic()
    yield rendered("header-physical-offsets", physical, "Leading junk already included in recorded offsets",
                   requirement="document-repairs", strict_valid=False, repairs=("file.header-offset",))
    for trailer in [True, False]:
        builder = base()
        if trailer:
            builder.data += b"trailer\n<< /Root 1 0 R /Size 5 >>\n"
        builder.data += b"%%EOF\n"
        yield rendered("rebuild-trailer" if trailer else "rebuild-catalog", bytes(builder.data),
                       "Reconstruction from complete objects with a retained trailer or Catalog",
                       requirement="document-repairs", strict_valid=False,
                       repairs=("xref.rebuilt",) if trailer else ("xref.rebuilt", "trailer.reconstructed"))
    builder = Builder()
    builder.obj(4, OBJECTS[3])
    builder.packed()
    builder.data += b"trailer\n<< /Root 1 0 R /Size 6 >>\n%%EOF\n"
    yield rendered("rebuild-object-stream", bytes(builder.data), "Reconstruction discovers compressed objects",
                   requirement="document-repairs", strict_valid=False, repairs=("xref.rebuilt",))
    damaged = list(OBJECTS)
    damaged[3] = stream(CONTENT).replace(b"/Length %d" % len(CONTENT), b"/Length 3")
    yield rendered("short-length", document(damaged), "Underreported stream length has a unique endstream terminator",
                   requirement="document-repairs", strict_valid=False, repairs=("stream.length-corrected",))
    yield rendered("trailing-junk", document(OBJECTS, tail=b"junk\n"), "Data after final EOF is reported",
                   requirement="document-repairs", strict_valid=False, repairs=("file.trailing-bytes",))
    yield rendered("truncated", document(OBJECTS)[:-6], "Missing EOF triggers reconstruction without losing complete objects",
                   requirement="document-repairs", strict_valid=False, repairs=("xref.rebuilt",))
    builder = base()
    invalid = builder.xref().replace(b"/W [1 4 2]", b"/W [1 4 9]")
    yield rendered("xref-truncated-fields", invalid, "Malformed W fields trigger reconstruction from complete objects",
                   requirement="xref-streams", strict_valid=False, repairs=("xref.rebuilt",))

    for revision in [2, 3, 4]:
        yield rendered(f"security-r{revision}", encrypted(revision),
                       "Independently derived Standard-handler RC4 file with empty user password and encrypted Marker string",
                       requirement="document-security", extra={"owner_password": "owner", "marker": "independent"})
    yield rendered("security-crypt-identity", encrypted(4, identity=True),
                   "Explicit Crypt Identity overrides encrypted default streams; strings still use RC4",
                   requirement="document-security", extra={"owner_password": "owner", "marker": "independent"})

    for name, damaged in recovery_cases():
        yield rendered(name, damaged, "Primary structural failure falls back to reported reconstruction",
                       requirement="document-repairs", strict_valid=False, repairs=("xref.rebuilt",))
    original = document(OBJECTS)
    trailer_at = original.index(b"trailer")
    yield rendered("rebuild-truncated-trailer", original[:trailer_at + 16],
                   "An incomplete trailer uses the scanned Catalog as a last resort",
                   requirement="document-repairs", strict_valid=False,
                   repairs=("xref.rebuilt", "trailer.reconstructed"))
    builder = Builder()
    builder.obj(4, OBJECTS[3])
    builder.packed()
    builder.data += b"startxref\n999999\n"
    yield rendered("rebuild-compressed-catalog", bytes(builder.data),
                   "A compressed Catalog is found after rebuilding an absent trailer",
                   requirement="document-repairs", strict_valid=False,
                   repairs=("xref.rebuilt", "trailer.reconstructed"))
    data = document([*OBJECTS, b"<< /Producer (independent) >>"],
                    trailer_extra=b"/Info 5 0 R /ID [(first) (second)] ")
    yield rendered("rebuild-identities", replace_startxref(data, 999999),
                   "A parseable trailer preserves Root, Info and both ID strings",
                   requirement="document-repairs", strict_valid=False, repairs=("xref.rebuilt",))
    builder = base()
    builder.obj(7, b"<< /Producer (older) >>")
    builder.obj(8, b"<< /Producer (newer) >>")
    builder.classic(extra=b"/Info 7 0 R /ID [(older) (old)] ")
    updated, _ = builder.classic(extra=b"/Prev 999999 /Info 8 0 R /ID [(newer) (new)] ", root=False)
    yield rendered("rebuild-last-trailer", updated,
                   "Last parseable trailer fields win and omitted Root inherits",
                   requirement="document-repairs", strict_valid=False, repairs=("xref.rebuilt",))
    builder = base()
    builder.obj(7, b"(99 0 obj\nnull\nendobj\ntrailer << /Root 99 0 R >>)")
    builder.obj(8, stream(b"99 0 obj\nnull\nendobj\ntrailer << /Encrypt 99 0 R >>"))
    opaque, _ = builder.classic()
    yield rendered("rebuild-opaque-payloads", replace_startxref(opaque, 999999),
                   "Object headers and trailers inside strings or stream payloads remain opaque",
                   requirement="document-repairs", strict_valid=False, repairs=("xref.rebuilt",))
    previous = document([*OBJECTS, b"<< /Producer (independent) >>"],
                        trailer_extra=b"/Info 5 0 R /ID [(first) (second)] ")
    yield rendered("rebuild-malformed-last-trailer", previous + b"trailer\n<< /Root [broken\nstartxref\n",
                   "An unparsable final trailer does not replace the last parseable identity fields",
                   requirement="document-repairs", strict_valid=False, repairs=("xref.rebuilt",))
    builder = base()
    builder.entries[1] = builder.entries[4]
    damaged, _ = builder.classic()
    yield rendered("rebuild-catalog-offset", damaged,
                   "A parseable xref with an unusable Catalog offset reconstructs during opening",
                   requirement="document-repairs", strict_valid=False, repairs=("xref.rebuilt",))
    yield rendered("rebuild-unusable-root", document(OBJECTS).replace(b"/Root 1 0 R", b"/Root 99 0 R"),
                   "An undefined Root uses the scanned Catalog while preserving other trailer fields",
                   requirement="document-repairs", strict_valid=False,
                   repairs=("xref.rebuilt", "trailer.reconstructed"))
    for name, data in [
        ("rebuild-no-catalog", HEADER + b"1 0 obj\nnull\nendobj\n%%EOF\n"),
        ("rebuild-incomplete-object", HEADER + b"1 0 obj\n<< /Type /Catalog >>\n"),
    ]:
        yield Case(name, data, INVALID, ("document-repairs",),
                   "Structural reconstruction cannot invent a missing complete Catalog", pages=None, strict_valid=False)
    data = bytearray(base().xref(flate=True))
    start = data.index(b"\nstream\n", data.index(b"/Type /XRef")) + len(b"\nstream\n")
    data[start] = 0  # Independently invalid RFC 1950 compression-method header.
    yield rendered("rebuild-corrupt-flate-stream", bytes(data),
                   "A decoder syntax error triggers reconstruction from the parseable xref dictionary",
                   requirement="document-repairs", strict_valid=False, repairs=("xref.rebuilt",),
                   extra={})
    for compressed in [False, True]:
        data = encrypted(4, compressed=compressed)
        damaged = data.replace(b"/W [1 4 2]", b"/W [1 4 9]") if compressed else replace_startxref(data, 999999)
        yield rendered("security-rebuilt-object-stream" if compressed else "security-rebuilt",
                       damaged, "Recovered Encrypt and ID authenticate before any object-stream decoding",
                       requirement="document-security", strict_valid=False, repairs=("xref.rebuilt",),
                       extra={"owner_password": "owner", "marker": "independent"})

    shared = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 4 /MediaBox [0 0 64 64] /Resources 7 0 R >>",
        b"<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>",
        b"<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>",
        b"<< /Type /Pages /Parent 2 0 R /Kids [8 0 R 9 0 R] /Count 2 /Resources << /ProcSet [/PDF] >> >>",
        stream(CONTENT),
        b"<< /ProcSet [/PDF] /ExtGState << >> >>",
        b"<< /Type /Page /Parent 5 0 R /Contents 6 0 R >>",
        b"<< /Type /Page /Parent 5 0 R /Contents 6 0 R /Resources 7 0 R >>",
    ]
    yield Case("pages-shared-resources", document(shared), OPEN, ("page-tree",),
               "Inherited indirect, inherited direct and repeated page resources (7.7.3.4, Table 31)", pages=4)

    yield from field_recovery_cases()
    yield from structural_recovery_cases()


def replace_startxref(data, offset):
    marker = data.rindex(b"startxref") + len(b"startxref")
    end = data.index(b"%%EOF", marker) if b"%%EOF" in data[marker:] else len(data)
    return data[:marker] + b"\n%d\n" % offset + data[end:]


def recovery_cases():
    original = document(OBJECTS)
    marker = original.rindex(b"startxref")
    yield "rebuild-missing-startxref", original[:marker] + b"%%EOF\n"
    yield "rebuild-startxref-outside", replace_startxref(original, 999999)
    yield "rebuild-startxref-object", replace_startxref(original, len(HEADER))
    yield "rebuild-startxref-junk", replace_startxref(original, 0)
    damaged = bytearray(original)
    at = damaged.index(b"00000 n", damaged.index(b"xref"))
    damaged[at + 6] = ord("x")
    yield "rebuild-malformed-table", bytes(damaged)
    builder = base()
    yield "rebuild-stream-length", builder.xref().replace(b"/Length 49", b"/Length 01")
    builder = base()
    previous, _ = builder.classic(extra=b"/Prev 999999 ")
    yield "rebuild-prev-missing", previous
    builder = base()
    previous, _ = builder.classic(extra=b"/Prev (broken) ")
    yield "rebuild-prev-malformed", previous
    yield "rebuild-truncated-startxref", original[:marker] + b"startxref\n9"


def repaired_document(name, pdf, tag, description, oracle=RED, **extra):
    requirement = "security-field-recovery" if tag.startswith("security.") else "catalog-type-recovery" if tag.startswith("catalog.") else "stream-boundary-recovery" if tag == "stream.eol" else "document-header-recovery"
    return Case(name, pdf, RENDER, ("document-repairs", requirement), description,
                oracle=oracle, strict_valid=False, repairs=(tag,), extra=extra)


def literal_binary(data):
    # Delimiters and backslashes are escaped; physical CR and LF are intentional.
    return b"(" + data.replace(b"\\", b"\\\\").replace(b"(", b"\\(").replace(b")", b"\\)") + b")"


def security_pdf(params, key, method, *, literals=False, padding=0, unsigned=False):
    from .crypto import encrypt, encryption_dictionary, hex_string, ID
    parameters = dict(params)
    if padding:
        for name in ["owner_hash", "user_hash"]:
            parameters[name] += bytes(padding)
    dictionary = encryption_dictionary(parameters, method, False)
    if literals:
        for field in ["owner_hash", "user_hash"]:
            dictionary = dictionary.replace(hex_string(parameters[field]), literal_binary(parameters[field]))
    if unsigned:
        dictionary = dictionary.replace(b"/P %d " % params["permissions"], b"/P %d " % (params["permissions"] + (1 << 32)))
    return document([*OBJECTS[:3], stream(encrypt(key, method, 4, CONTENT)), dictionary],
                    b"/Encrypt 5 0 R /ID [" + hex_string(ID) + b" " + hex_string(ID) + b"] ")


def field_recovery_cases():
    for name, suffix in [("header-text", b" Sharp Scanned ImagePDF"), ("header-space", b" "), ("header-tab", b"\t")]:
        builder = Builder()
        builder.data = bytearray(HEADER.replace(b"%PDF-1.7\n", b"%PDF-1.7" + suffix + b"\r\n"))
        for number, body in enumerate(OBJECTS, 1):
            builder.obj(number, body)
        data, _ = builder.classic()
        yield repaired_document(name, data, "file.header-format", "Trailing header-line text/space is a reported repair; physical offsets are unchanged")
    objects = list(OBJECTS)
    objects[0] = b"<< /Pages 2 0 R >>"
    yield repaired_document("catalog-type-missing", document(objects), "catalog.type-missing",
                            "Only a missing Type with a reference to a typed Pages node identifies a Catalog")
    builder = Builder()
    for number, body in enumerate(objects, 1):
        builder.obj(number, body)
    builder.data += b"%%EOF\n"
    yield Case("catalog-type-missing-rebuilt", bytes(builder.data), RENDER, ("document-repairs", "catalog-type-recovery"),
               "Reconstruction recognizes a Catalog through the typed Pages target",
               oracle=RED, strict_valid=False, repairs=("xref.rebuilt", "trailer.reconstructed", "catalog.type-missing"))
    objects = list(OBJECTS)
    objects[3] = stream(CONTENT).replace(b"stream\n", b"stream\r", 1)
    yield repaired_document("stream-bare-cr", document(objects), "stream.eol",
                            "Bare CR separates the stream keyword and payload; LF/CRLF remain the strict forms")
    builder = Builder()
    builder.obj(5, b"<< /Length 6 0 R >>\nstream\nopaqueendstream")
    builder.obj(6, b"6")
    for number, body in enumerate(OBJECTS, 1):
        builder.obj(number, body)
    data, _ = builder.classic()
    data = data.replace(b"0000000000 65535 f", b"0000000000 65535 n")
    yield Case("rebuild-indirect-stream-boundary", data, RENDER, ("document-repairs", "stream-boundary-recovery"),
               "Damaged xref reconstruction must not swallow following objects when an indirect-length stream lacks a pre-endstream EOL",
               oracle=RED, strict_valid=False, repairs=("xref.rebuilt", "xref.stream-boundary"))
    from .crypto import parameters, modern_hash, AES
    params, key = parameters(3, b"", b"owner", permissions=-1036)
    yield repaired_document("permissions-unsigned", security_pdf(params, key, "V2", unsigned=True),
                            "security.permissions-unsigned", "Unsigned 32-bit spelling preserves the signed permission bits during key derivation")
    params, key = parameters(6, b"", b"owner")
    yield repaired_document("hash-zero-padding", security_pdf(params, key, "AESV3", padding=79),
                            "security.hash-padding", "Only excess zero bytes beyond a fixed-length modern hash are discarded; authentication still must pass")
    # Both validation salts intentionally contain physical CRLF. No search for
    # an accidental digest byte pattern, and no result derived from Pagura.
    uv, uk, ov, ok = b"\r\nsalt00", b"usaltkey", b"\r\nsalt11", b"osaltkey"
    user = modern_hash(6, b"", uv) + uv + uk
    params = dict(params, user_hash=user, owner_hash=modern_hash(6, b"owner", ov, user) + ov + ok,
                  user_encrypted_key=AES(modern_hash(6, b"", uk)).cbc(key),
                  owner_encrypted_key=AES(modern_hash(6, b"owner", ok, user)).cbc(key))
    yield repaired_document("hash-literal-eol", security_pdf(params, key, "AESV3", literals=True),
                            "security.literal-hash", "Binary literal O/U fields recover their original CRLF bytes only after a length mismatch")
    for name, catalog in [("catalog-type-wrong", b"<< /Type /Page /Pages 2 0 R >>"),
                          ("catalog-pages-wrong-type", b"<< /Pages 3 0 R >>")]:
        yield Case(name, document([catalog, *OBJECTS[1:]]), INVALID, ("document-repairs",),
                   "Missing-Type recovery cannot excuse an explicitly wrong Type or a non-Pages target", pages=None, strict_valid=False)


def structural_recovery_cases():
    for name, suffix in [("header-nul", b"\x00"), ("header-extra-digit", b"0"), ("header-attached-text", b"damaged")]:
        builder = Builder()
        builder.data = bytearray(HEADER.replace(b"%PDF-1.7\n", b"%PDF-1.7" + suffix + b"\n"))
        for number, body in enumerate(OBJECTS, 1):
            builder.obj(number, body)
        data, _ = builder.classic()
        yield repaired_document(name, data, "file.header-format", "Extra bytes after a complete version are reported without changing physical offsets")
    for version in [b"1.8", b"1.9", b"2.9"]:
        yield rendered("header-version-" + version.decode().replace(".", "-"),
                       document(OBJECTS).replace(b"%PDF-1.7", b"%PDF-" + version),
                       "Clause 7.5.2 permits a single minor-version digit after major version 1 or 2",
                       requirement="document-header-syntax")
    builder = Builder()
    for number, body in enumerate(OBJECTS, 1):
        builder.obj(number, body)
        if number == 1:
            del builder.data[-7:]  # Omit only the complete Catalog's terminator.
    data, _ = builder.classic()
    yield Case("object-end-missing", data, RENDER, ("object-boundary-recovery",),
               "A complete object followed immediately by another object header has an unambiguous missing terminator",
               oracle=RED, strict_valid=False, repairs=("object.end-missing",))
    objects = list(OBJECTS)
    objects[0] = b"<< /Type /Unrecognized /Pages 2 0 R >>"
    yield Case("catalog-type-invalid", document(objects), RENDER, ("catalog-type-recovery",),
               "An unknown Catalog Type can be inferred from its reference to a typed Pages root; known conflicting types remain errors",
               oracle=RED, strict_valid=False, repairs=("catalog.type-invalid",))
    objects = list(OBJECTS)
    objects[0] = b"<< /Pages 2 0 R >>"
    objects[1] = b"<< /Kids [3 0 R] /Count 1 >>"
    objects[2] = OBJECTS[2].replace(b"/Type /Page ", b"")
    yield Case("pages-type-missing", document(objects), RENDER, ("page-type-recovery", "catalog-type-recovery"),
               "Kids references distinguish a missing branch Type; leaves retain explicit MediaBox and content",
               oracle=RED, strict_valid=False, repairs=("catalog.type-missing", "pages.type-missing", "pages.type-missing"))
    objects = list(OBJECTS)
    objects[1] = b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 64 64] >>"
    objects[2] = b"<< /Parent 2 0 R /Resources << >> >>"
    yield Case("pages-leaf-untyped", document(objects), INVALID, ("page-type-recovery",),
               "An untyped node without Kids, Contents or its own MediaBox is not identifiable; inherited attributes are no evidence",
               pages=None, strict_valid=False)
    for name, length in [("stream-length-missing", b""), ("stream-length-negative", b"/Length -1"),
                         ("stream-length-undefined", b"/Length 99 0 R"), ("stream-length-outside", b"/Length 9999")]:
        objects = list(OBJECTS)
        objects[3] = b"<< " + length + b" >>\nstream\n" + CONTENT + b"\nendstream"
        yield Case(name, document(objects), RENDER, ("stream-boundary-recovery",),
                   "A unique line-delimited endstream/endobj within the physical object boundary recovers unusable Length",
                   oracle=RED, strict_valid=False, repairs=("stream.length-corrected",))
    from .crypto import parameters, encryption_dictionary, encrypt, hex_string, ID
    params, key = parameters(6, b"", b"owner")
    dictionary = encryption_dictionary(params, "AESV3", False)
    direct = b"/CF << /StdCF << /CFM /AESV3 /AuthEvent /DocOpen /Length 32 >> >>"
    assert direct in dictionary
    for name, replacement, following in [
        ("security-indirect-cf", b"/CF 6 0 R", [b"<< /StdCF 7 0 R >>", b"<< /CFM /AESV3 /Length 32 /AuthEvent /DocOpen >>"]),
        ("security-indirect-filter", b"/CF << /StdCF 6 0 R >>", [b"<< /CFM /AESV3 /Length 32 /AuthEvent /DocOpen >>"]),
    ]:
        objects = [*OBJECTS[:3], stream(encrypt(key, "AESV3", 4, CONTENT)), dictionary.replace(direct, replacement), *following]
        data = document(objects, b"/Encrypt 5 0 R /ID [" + hex_string(ID) + b" " + hex_string(ID) + b"] ")
        yield rendered(name, data, "Unencrypted crypt-filter dictionaries may be indirect (7.3.10, 7.6.2 and 7.6.6)",
                       requirement="security-indirect-filters", extra={"owner_password": "owner"})
