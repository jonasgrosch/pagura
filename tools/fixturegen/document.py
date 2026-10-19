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


def encrypted(revision, identity=False, compressed=False, direct=False,
              auth_event=None, attachment_only=False, user_password=b"", identity_attachments=False, omit_length=False):
    assert not (direct and compressed)
    assert not attachment_only or (revision == 4 and not compressed)
    assert not identity_attachments or attachment_only
    assert not omit_length or revision == 3
    padded_user = (user_password + PADDING)[:32]
    assert len(PADDING) == 32
    assert rc4(b"Key", b"Plaintext").hex() == "bbf316e8d940af0ad3"
    version = {2: 1, 3: 2, 4: 4}[revision]
    length = 5 if revision == 2 else 16
    digest = hashlib.md5((b"owner" + PADDING)[:32]).digest()
    if revision >= 3:
        for _ in range(50):
            digest = hashlib.md5(digest).digest()
    owner_key = digest[:length]
    owner = rc4(owner_key, padded_user)
    if revision >= 3:
        for iteration in range(1, 20):
            owner = rc4(bytes(value ^ iteration for value in owner_key), owner)
    file_id = bytes.fromhex("00112233445566778899aabbccddeeff")
    digest = hashlib.md5(padded_user + owner + struct.pack("<i", -4) + file_id).digest()
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
    if omit_length:
        encryption = encryption.replace(b"/Length %d " % (length * 8), b"", 1)
    if revision == 4:
        event = b"/AuthEvent /" + auth_event + b" " if auth_event else b""
        encryption += b"/CF << /StdCF << /CFM /" + (b"None" if identity_attachments else b"V2") + b" /Length 16 " + event + b">> >> "
        encryption += b"/StrF /Identity /StmF /Identity /EFF /StdCF " if attachment_only else b"/StrF /StdCF /StmF /StdCF "
    encryption += b">>"
    objects = list(OBJECTS)
    objects[0] = OBJECTS[0].replace(b"/Pages", b"/Marker <" + (b"independent" if attachment_only else encrypt(1, b"independent")).hex().encode() + b"> /Pages")
    objects[3] = stream(CONTENT if identity or attachment_only else encrypt(4, CONTENT),
                        b"/Filter /Crypt /DecodeParms << /Name /Identity >> " if identity else b"")
    if direct:
        inline = stream(encrypt(3, CONTENT), b"/Label <" + encrypt(3, b"inline").hex().encode() + b"> ")
        objects[2] = OBJECTS[2].replace(b"/Contents 4 0 R", b"/Contents " + inline)
    objects.append(encryption)
    if attachment_only:
        objects.append(stream(b"attached file bytes" if identity_attachments else encrypt(6, b"attached file bytes"), b"/Type /EmbeddedFile "))
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
    yield from bounded_loading_cases()
    yield from retained_cache_cases()
    yield from page_recovery_cases()
    yield from object_stream_recovery_cases()
    yield from direct_stream_cases()
    yield from authorization_event_cases()
    yield from key_length_cases()
    yield from security_dictionary_cases()
    yield from short_rc4_cases()
    yield from identifier_syntax_cases()
    yield from legacy_selector_cases()
    yield from catalog_identity_cases()
    yield from navigation_cases()
    from .document_forms import cases as form_cases
    yield from form_cases()


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


def bounded_loading_cases():
    for name, lengths in [("length-self-cycle", {4: 4}), ("length-mutual-cycle", {4: 5, 5: 4})]:
        objects = list(OBJECTS[:3])
        for number in sorted(lengths):
            objects.append(b"<< /Length %d 0 R >>\nstream\n" % lengths[number] + CONTENT + b"\nendstream")
        yield Case(name, document(objects), OPEN, ("document-loading-bounds",),
                   "Cyclic stream Length dependencies fail with a structured cycle error during page compilation",
                   extra={"render_error": "cycle"})
    for name, count, expected in [("length-chain-short", 3, RENDER), ("length-chain-depth", 70, OPEN)]:
        objects = [*OBJECTS[:3], b"<< /Length 5 0 R >>\nstream\n" + CONTENT + b"\nendstream"]
        objects.extend(b"%d 0 R" % (number + 1) for number in range(5, 5 + count - 1))
        objects.append(str(len(CONTENT)).encode())
        yield Case(name, document(objects), expected, ("document-loading-bounds",),
                   "Indirect Length aliases terminate at an integer or fail at the configured reference-depth limit",
                   oracle=RED if expected == RENDER else None,
                   extra={} if expected == RENDER else {"render_error": "limit"})
    objects = list(OBJECTS)
    objects[3] = stream(CONTENT.hex().encode() + b">extra", b"/Filter /ASCIIHexDecode ")
    yield Case("diagnostics-repeated-decode", document(objects), RENDER, ("document-diagnostic-bounds",),
               "Repeated decoding produces the same positioned repair and retains it once per document",
               oracle=RED, strict_valid=False, repairs=("filter.trailing-data",))


def retained_cache_cases():
    yield rendered("cache-large-values", document([*OBJECTS, *[b"(" + bytes([65 + i]) * 2048 + b")" for i in range(16)]]),
                   "Independent large unused string objects exercise aggregate cache accounting with lowered API budgets",
                   requirement="document-retained-memory")
    builder = base()
    builder.packed(number=5, values=[(6, b"(" + b"A" * 2048 + b")"), (7, b"(" + b"B" * 2048 + b")")])
    yield rendered("cache-lazy-object-stream", builder.xref(number=8),
                   "An object stream containing unused strings stays lazy; each retained decoded buffer is charged once",
                   requirement="document-retained-memory")


def page_recovery_cases():
    # Each malformed input is independently constructed with known page order,
    # geometry and repair identities; no reader supplies these expectations.
    def repaired(name, objects, repairs, requirement="page-graph-recovery", pages=1):
        return Case(name, document(objects), OPEN, (requirement,),
                    "Reported page recovery with strict rejection and independently specified leaves",
                    strict_valid=False, repairs=repairs, pages=pages)

    for name, page, tags in [
        ("pages-media-missing", OBJECTS[2].replace(b" /MediaBox [0 0 64 64]", b""), ("pages.media-box",)),
        ("pages-media-empty", OBJECTS[2].replace(b"[0 0 64 64]", b"[0 0 0 0]"), ("pages.media-box",)),
        ("pages-media-from-crop", OBJECTS[2].replace(b"/MediaBox", b"/CropBox"), ("pages.media-box",)),
        ("pages-crop-empty", OBJECTS[2].replace(b"/Resources", b"/CropBox [0 0 0 0] /Resources"), ("pages.crop-box",)),
        ("pages-crop-disjoint", OBJECTS[2].replace(b"/Resources", b"/CropBox [80 80 90 90] /Resources"), ("pages.crop-box",)),
    ]:
        objects = list(OBJECTS); objects[2] = page
        yield repaired(name, objects, tags, "page-box-recovery")
    for name, branch, page, tags, count in [
        ("pages-kids-invalid", b"<< /Type /Pages /Kids [null 99 0 R -1 3 0 R] /Count 1 >>", OBJECTS[2], ("pages.kids-invalid",), 1),
        ("pages-kids-single", b"<< /Type /Pages /Kids 3 0 R /Count 1 >>", OBJECTS[2], ("pages.kids-invalid",), 1),
        ("pages-kids-rebuilt", b"<< /Type /Pages /Count 1 >>", OBJECTS[2], ("pages.kids-rebuilt",), 1),
        ("pages-root-untyped-backlinks", b"<< /Count 1 >>", OBJECTS[2], ("pages.kids-rebuilt", "pages.type-missing"), 1),
        ("pages-repeat-leaf", b"<< /Type /Pages /Kids [3 0 R 3 0 R] /Count 2 >>", OBJECTS[2], ("pages.repeated-node",), 2),
        ("pages-cycle-usable", b"<< /Type /Pages /Kids [2 0 R 3 0 R] /Count 1 >>", OBJECTS[2], ("pages.cycle",), 1),
        ("pages-parent-mismatch", OBJECTS[1], OBJECTS[2].replace(b"/Parent 2 0 R", b"/Parent 99 0 R"), ("pages.parent-mismatch",), 1),
        ("pages-template-kid", OBJECTS[1], OBJECTS[2].replace(b"/Type /Page ", b"/Type /Template "), ("pages.type-invalid",), 1),
    ]:
        yield repaired(name, [OBJECTS[0], branch, page, OBJECTS[3]], tags, pages=count)
    objects = list(OBJECTS)
    objects[1] = b"<< /Type /Pages /Kids [5 0 R 5 0 R] /Count 2 >>"
    objects[2] = OBJECTS[2].replace(b"/Parent 2 0 R", b"/Parent 5 0 R")
    objects.append(b"<< /Type /Pages /Parent 2 0 R /Kids [3 0 R] /Count 1 >>")
    yield repaired("pages-repeat-branch", objects, ("pages.repeated-node", "pages.repeated-node"), pages=2)
    objects = list(OBJECTS)
    first = OBJECTS[2].replace(b"/Resources << >>", b"/Resources << /ProcSet [/PDF] >>")
    second = OBJECTS[2].replace(b"/Resources << >>", b"/Resources << /ExtGState << >> >>")
    objects[1] = b"<< /Type /Pages /Kids [" + first + second + b"] /Count 2 >>"
    yield repaired("pages-direct-kids", objects, ("pages.direct-kid",), pages=2)
    for name, branch in [
        ("pages-cycle-only", b"<< /Type /Pages /Kids [2 0 R] /Count 1 >>"),
        ("pages-kids-all-invalid", b"<< /Type /Pages /Kids [null 99 0 R] /Count 1 >>"),
    ]:
        yield Case(name, document([OBJECTS[0], branch]), INVALID, ("page-graph-recovery",),
                   "A damaged tree without identifiable leaves cannot become an empty document", strict_valid=False)

    for name, page, tags in [
        ("syntax-key-solidus", OBJECTS[2].replace(b"/Resources << >>", b"/Resources << ProcSet [/PDF] >>"), ("syntax.key-solidus",)),
        ("syntax-name-escape", OBJECTS[2].replace(b"/Resources << >>", b"/Resources << /Bad#Key true >>"), ("syntax.name-escape",)),
        ("syntax-reference-zero", OBJECTS[2].replace(b"/Resources", b"/CropBox 0 0 R /Resources"), ("syntax.reference-zero",)),
        ("syntax-dictionary-closer", OBJECTS[2][:-2] + b") >>", ("syntax.dictionary-closer",)),
        ("syntax-dictionary-opener", b"<< " + OBJECTS[2], ("syntax.dictionary-opener",)),
    ]:
        objects = list(OBJECTS); objects[2] = page
        yield repaired(name, objects, tags, "document-lexical-recovery")
    # Glue complete object 3's endobj to the next header. Recompute every
    # subsequent offset rather than obtaining offsets from the implementation.
    builder = Builder()
    for number, body in enumerate(OBJECTS, 1):
        builder.obj(number, body)
        if number == 3:
            del builder.data[-1:]
    data, _ = builder.classic()
    yield Case("object-separator-missing", data, RENDER, ("document-lexical-recovery",),
               "A proven following object header permits recovery of a missing endobj separator",
               oracle=RED, strict_valid=False, repairs=("object.separator-missing",))
    builder = base()
    builder.entries[3], builder.entries[4] = builder.entries[4], builder.entries[3]
    data, _ = builder.classic()
    yield Case("xref-swapped-offsets", data, RENDER, ("xref-lazy-offset-validation",),
               "Swapped distant offsets reconstruct once before lazy object caching",
               oracle=RED, strict_valid=False, repairs=("xref.rebuilt",))


def object_stream_recovery_cases():
    for name, kind in [("object-stream-type-invalid", b"Other"), ("object-stream-type-conflict", b"Font")]:
        builder = Builder()
        builder.obj(4, OBJECTS[3])
        builder.packed()
        at = builder.data.index(b"/Type /ObjStm")
        # Equal-width replacement preserves every independently computed offset.
        field = b"/Type /ObjStm"
        builder.data[at:at+len(field)] = (b"/Type /" + kind).ljust(len(field))
        if kind == b"Other":
            yield Case(name, builder.xref(), RENDER, ("object-stream-header-recovery",),
                       "An unknown Type is inferred only after validating the xref-selected N/First header",
                       oracle=RED, strict_valid=False, repairs=("object.stream-type",))
        else:
            yield Case(name, builder.xref(), INVALID, ("object-stream-header-recovery",),
                       "A known conflicting dictionary role cannot be inferred as an object stream", strict_valid=False)
    builder = Builder()
    builder.obj(4, OBJECTS[3])
    bodies = bytearray(); pairs = [b"5 0"]
    for number, body in enumerate(OBJECTS[:3], 1):
        pairs.append(b"%d %d" % (number, len(bodies)))
        bodies += body + b"\n"
        builder.entries[number] = (2, 5, number)
    header = b" ".join(pairs) + b" "
    builder.obj(5, stream(header + bodies, b"/Type /ObjStm /N 4 /First %d " % len(header)))
    builder.entries[5] = (2, 5, 0)
    yield Case("object-stream-self-entry", builder.xref(), RENDER, ("object-stream-header-recovery",),
               "The first self entry aliases the first real value; physical stream and original indexes survive reconstruction",
               oracle=RED, strict_valid=False, repairs=("xref.rebuilt", "object.stream-self-entry"))

    invalid = Builder()
    invalid.obj(4, OBJECTS[3])
    bad_header = b"9 0 " + b" ".join(pairs[1:]) + b" "
    invalid.obj(5, stream(bad_header + bodies, b"/Type /ObjStm /N 4 /First %d " % len(bad_header)))
    for number in range(1, 4):
        invalid.entries[number] = (2, 5, number)
    invalid.entries[9] = (2, 5, 0)
    yield Case("object-stream-offset-alias", invalid.xref(), INVALID, ("object-stream-header-recovery",),
               "Two real objects sharing an offset cannot use the narrowly defined self-entry repair", strict_valid=False)


def direct_stream_cases():
    inline = stream(CONTENT)
    page = OBJECTS[2].replace(b"/Contents 4 0 R", b"/Contents " + inline)
    data = document([OBJECTS[0], OBJECTS[1], page, OBJECTS[3]])
    for name, pdf, repairs in [
        ("direct-stream-contents", data, ("syntax.direct-stream",)),
        ("direct-stream-rebuilt", replace_startxref(data, 999999), ("xref.rebuilt", "syntax.direct-stream")),
    ]:
        yield Case(name, pdf, RENDER, ("direct-stream-recovery",),
                   "A direct content stream retains its dictionary and original payload range",
                   oracle=RED, strict_valid=False, repairs=repairs)
    array = b"[" + inline + b" " + stream(b"") + b"]"
    for name, objects in [
        ("direct-stream-array", [OBJECTS[0], OBJECTS[1], OBJECTS[2].replace(b"4 0 R", array), OBJECTS[3]]),
        ("direct-stream-indirect-array", [OBJECTS[0], OBJECTS[1], OBJECTS[2], array]),
    ]:
        yield Case(name, document(objects), RENDER, ("direct-stream-recovery",),
                   "Direct stream array elements keep distinct ranges in content order, including indirect arrays",
                   oracle=RED, strict_valid=False, repairs=("syntax.direct-stream", "syntax.direct-stream"))
    branch = b"<< /Type /Pages /Kids [" + page + b"] /Count 1 >>"
    for name, objects in [
        ("direct-stream-direct-page", [OBJECTS[0], branch, OBJECTS[2], OBJECTS[3]]),
        ("direct-stream-indirect-kids", [OBJECTS[0], b"<< /Type /Pages /Kids 5 0 R /Count 1 >>", OBJECTS[2], OBJECTS[3], b"[" + page + b"]"]),
    ]:
        yield Case(name, document(objects), RENDER, ("direct-stream-recovery", "page-graph-recovery"),
                   "A direct page's source path identifies its own embedded content in the containing object",
                   oracle=RED, strict_valid=False, repairs=("pages.direct-kid", "syntax.direct-stream"))
    image = stream(bytes([255]), b"/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray ")
    branch = OBJECTS[1][:-2] + b"/Resources << /XObject << /Unused " + image + b">> >> >>"
    inherited = OBJECTS[2].replace(b"/Resources << >>", b"")
    yield Case("direct-stream-resources", document([OBJECTS[0], branch, inherited, OBJECTS[3]]), RENDER,
               ("direct-stream-recovery",), "A nested resource payload is skipped structurally and stays in the original bytes",
               oracle=RED, strict_valid=False, repairs=("syntax.direct-stream",))
    yield Case("direct-stream-encrypted", encrypted(2, direct=True), RENDER, ("direct-stream-recovery",),
               "A direct payload and its header strings use the containing object's encryption key",
               oracle=RED, strict_valid=False, repairs=("syntax.direct-stream",),
               extra={"owner_password": "owner", "marker": "independent"})
    short = inline.replace(b"/Length %d" % len(CONTENT), b"/Length 1")
    yield Case("direct-stream-length", document([OBJECTS[0], OBJECTS[1], OBJECTS[2].replace(b"4 0 R", short), OBJECTS[3]]),
               RENDER, ("direct-stream-recovery",), "One validated dictionary continuation bounds a wrong direct stream length",
               oracle=RED, strict_valid=False, repairs=("syntax.direct-stream", "stream.length-corrected"))
    for name, contents in [
        ("direct-stream-ambiguous", b"[" + short + b" " + stream(b"") + b"]"),
        ("direct-stream-duplicate-key", inline + b" /Contents 4 0 R"),
        ("direct-stream-no-terminator", inline.replace(b"endstream", b"")),
    ]:
        yield Case(name, document([OBJECTS[0], OBJECTS[1], OBJECTS[2].replace(b"4 0 R", contents), OBJECTS[3]]), INVALID,
                   ("direct-stream-recovery",), "Ambiguous, shadowed or unterminated payloads cannot be recovered by inventing boundaries",
                   strict_valid=False)

    encrypted_bytes = encrypted(2)
    # Rebuild with the independently generated security fields while adding an
    # illegal embedded stream to Encrypt; no security data is guessed.
    builder = Builder()
    marker = encrypted_bytes.index(b"5 0 obj\n") + len(b"5 0 obj\n")
    encryption = encrypted_bytes[marker:encrypted_bytes.index(b"\nendobj", marker)]
    for number, body in enumerate(OBJECTS, 1):
        builder.obj(number, body)
    builder.obj(5, encryption[:-2] + b" /Invalid " + stream(b"") + b" >>")
    identifier = bytes.fromhex("00112233445566778899aabbccddeeff").hex().encode()
    bad, _ = builder.classic(extra=b"/Encrypt 5 0 R /ID [<" + identifier + b"><" + identifier + b">] ")
    yield Case("direct-stream-encrypt-dictionary", bad, INVALID, ("direct-stream-recovery",),
               "Security initialization cannot load embedded payload dependencies before the key exists", strict_valid=False)


def authorization_event_cases():
    yield Case("security-embedded-identity-filter", encrypted(4, attachment_only=True, auth_event=b"EFOpen", user_password=b"attachment", identity_attachments=True),
               INVALID, ("security-authorization-events",),
               "None delegates to the Standard handler and cannot silently act as Identity",
               strict_valid=False, extra={"open_error": "unsupported"})
    yield Case("security-embedded-event", encrypted(4, attachment_only=True, auth_event=b"EFOpen", user_password=b"attachment"),
               RENDER, ("security-authorization-events",),
               "EFOpen-only Standard filters are a reported extension; Identity page data remains readable while embedded bytes stay protected",
               oracle=RED, strict_valid=False, repairs=("security.crypt-filter-event",), extra={"owner_password": "owner", "marker": "independent"})
    yield Case("security-default-event-override", encrypted(4, auth_event=b"EFOpen"), RENDER,
               ("security-authorization-events",),
               "Table 25 requires StrF and StmF to use DocOpen even when their filter declares EFOpen",
               oracle=RED, extra={"owner_password": "owner", "marker": "independent"})
    yield Case("security-embedded-doc-open", encrypted(4, attachment_only=True, user_password=b"attachment"),
               INVALID, ("security-authorization-events",),
               "A crypt filter's default DocOpen event requires a password even if it encrypts only an attachment",
               strict_valid=False, extra={"open_error": "password"})


def key_length_cases():
    yield Case("security-length-missing", encrypted(3, omit_length=True), RENDER,
               ("security-key-length-recovery",),
               "Missing Length selects a wrong 40-bit default; exactly one other legal size authenticates the supplied credential",
               oracle=RED, strict_valid=False, repairs=("security.key-length",),
               extra={"owner_password": "owner", "marker": "independent"})
    yield Case("security-length-missing-password", encrypted(3, omit_length=True, user_password=b"view"), INVALID,
               ("security-key-length-recovery",),
               "Changing a missing key length cannot authenticate a wrong credential",
               strict_valid=False, extra={"open_error": "password"})


def catalog_identity_cases():
    for name, body, valid in [
        ("catalog-number-zero", OBJECTS[0], True),
        ("catalog-number-zero-conflict", b"<< /Type /XObject /Pages 2 0 R >>", False),
    ]:
        builder = Builder()
        for number, value in enumerate([body, *OBJECTS[1:]], 1):
            offset = len(builder.data)
            builder.obj(0 if number == 1 else number, value)
            if number == 1:
                builder.entries[0] = (0, 0, 65535)
                builder.entries[1] = (1, offset, 0)
        data, _ = builder.classic()
        yield Case(name, data, RENDER if valid else INVALID, ("catalog-identity-recovery",),
                   "Only an exact positive Root xref slot can identify a complete zero-numbered plaintext Catalog",
                   oracle=RED if valid else None, strict_valid=False,
                   repairs=("catalog.object-number",) if valid else ())
    data = encrypted(2).replace(b"1 0 obj", b"0 0 obj", 1)
    yield Case("catalog-number-zero-encrypted", data, INVALID, ("catalog-identity-recovery",),
               "A zero-numbered encrypted Catalog has no unambiguous object-key identity", strict_valid=False)


def security_dictionary_cases():
    from .crypto import parameters, encrypted_objects, ID, hex_string
    oracle = opaque_rectangles(64, 64, [((8, 8, 48, 32), (255, 0, 0)), ((0, 0, 8, 8), (0, 0, 255))])
    p, key = parameters(4, b"", b"owner-secret")
    base = encrypted_objects(p, key, "AESV2")
    trailer = b"/Encrypt 5 0 R /ID [" + hex_string(ID) + b" " + hex_string(ID) + b"] "
    for name, change, repairs in [
        ("security-identity-shadow", lambda d: d.replace(b"/CF <<", b"/CF << /Identity 7 ", 1), ()),
        ("security-auth-event-override", lambda d: d.replace(b"/AuthEvent /DocOpen", b"/AuthEvent /EFOpen"), ()),
        ("security-v4-default-length", lambda d: d.replace(b"/Length 128 ", b"", 1), ()),
        ("security-filter-type-null", lambda d: d.replace(b"/CFM /AESV2", b"/Type null /CFM /AESV2"), ()),
        ("security-auth-event-ignored-invalid", lambda d: d.replace(b"/AuthEvent /DocOpen", b"/AuthEvent /Wrong"), ()),
        ("security-filter-length-null", lambda d: d.replace(b"/Length 16 ", b"/Length null ", 1), ("security.crypt-filter-length",)),
        ("security-filter-type", lambda d: d.replace(b"/CFM /AESV2", b"/Type /Wrong /CFM /AESV2"), ("security.crypt-filter-type",)),
        ("security-filter-length", lambda d: d.replace(b"/Length 16 ", b"", 1), ("security.crypt-filter-length",)),
        ("security-filter-name", lambda d: d.replace(b"StdCF", b"OtherCF"), ("security.crypt-filter-name",)),
    ]:
        objects = list(base); objects[4] = change(objects[4])
        yield Case(name, document(objects, trailer), RENDER, ("security-dictionary-policy",),
                   "Tables 20 and 25 determine filter defaults; nonconforming dictionary fields are explicitly qualified",
                   oracle=oracle, repairs=repairs, strict_valid=not repairs)
    objects = encrypted_objects(p, key, "Identity")
    objects[4] = objects[4].replace(b"/CFM /Identity", b"/CFM /None")
    # Identity uses no CF entry in the independent encoder. Use the original
    # AES dictionary with a delegated None method, retaining harmless plaintext.
    objects[4] = base[4].replace(b"/CFM /AESV2", b"/CFM /None")
    yield Case("security-handler-delegation", document(objects, trailer), INVALID, ("security-dictionary-policy",),
               "None delegates to the handler and does not specify a cipher for Standard", strict_valid=False,
               extra={"open_error": "unsupported"})
    p, key = parameters(4, b"", b"owner-secret", metadata=False)
    objects = encrypted_objects(p, key, "AESV2")
    from .crypto import encrypt
    objects[0] = objects[0].replace(b" >>", b" /OtherMetadata 7 0 R >>", 1)
    objects.append(stream(encrypt(key, "AESV2", 7, b"private metadata"), b"/Type /Metadata /Subtype /XML "))
    yield Case("security-object-metadata", document(objects, trailer), RENDER, ("security-metadata-scope",),
               "Only catalog Metadata is exempt; the unrelated Metadata stream remains encrypted", oracle=oracle)
    p, key = parameters(2, b"", b"owner-secret")
    objects = encrypted_objects(p, key, "V2")
    objects[4] = objects[4][:-2] + b" /EncryptMetadata false >>"
    yield Case("security-v1-metadata", document(objects, trailer), RENDER, ("security-metadata-scope",),
               "EncryptMetadata is ignored for V=1; document metadata still decrypts", oracle=oracle)
    p, key = parameters(6, b"", b"owner-secret")
    base = encrypted_objects(p, key, "AESV3")
    for name, ids in [
        ("security-id-missing", b""),
        ("security-id-single", b"/ID ["+hex_string(ID)+b"] "),
        ("security-id-short", b"/ID [<00> <00>] "),
    ]:
        yield Case(name, document(base, b"/Encrypt 5 0 R " + ids), RENDER, ("security-identifiers",),
                   "Table 15 requires two direct 16-byte identifiers; R6 authentication does not depend on them",
                   oracle=oracle, strict_valid=False, repairs=("security.document-id",))
    objects=list(base)
    objects[4]=objects[4].replace(b"/P -4 ",b"/P -8 ")
    yield Case("security-permissions-inconsistent", document(objects, trailer), RENDER, ("security-permissions-validation",),
               "Algorithm 13 consistency differs from password authentication and is reported after a valid key is obtained",
               oracle=oracle, strict_valid=False, repairs=("security.perms-mismatch",))


def short_rc4_cases():
    from .crypto import parameters, rc4, ID, vector, PLAIN
    for length in [5, 6]:
        p, key = parameters(4, b"", b"owner", metadata=False, length=length)
        # Generation-aware known-answer data use independent MD5 and RC4;
        # ordinary authenticate must still reject this nonstandard R4 key size.
        raw = vector(f"r4-zero-padded-{length*8}", p, key, b"", b"owner", "V2")
        files = dict(raw.files)
        object_key = hashlib.md5(key.ljust(16, b"\0") + bytes([4, 3, 2, 1, 2])).digest()
        files[".cipher"] = rc4(object_key, PLAIN)
        from .model import Vector
        yield Vector(raw.name, files, ("security-short-rc4",),
                     "Independently derived 16-byte zero-padded RC4 object key, object 0x01020304 generation 513")
        def encode(number, data):
            digest=hashlib.md5(key.ljust(16,b"\0")+number.to_bytes(3,"little")+bytes(2)).digest()
            return rc4(digest,data)
        builder=Builder();builder.obj(4,stream(encode(4,CONTENT)))
        bodies=bytearray();pairs=[]
        for number,body in enumerate(OBJECTS[:3],1):
            pairs.append(b"%d %d"%(number,len(bodies)));bodies+=body+b"\n";builder.entries[number]=(2,5,number-1)
        header=b" ".join(pairs)+b" "
        builder.obj(5,stream(encode(5,zlib.compress(header+bodies)),b"/Type /ObjStm /N 3 /First %d /Filter /FlateDecode "%len(header)))
        builder.obj(6,b"<< /Title <"+encode(6,b"independent").hex().encode()+b"> >>")
        from .crypto import encryption_dictionary
        builder.obj(7,encryption_dictionary(p,"V2",False))
        ids=ID.hex().encode()
        data=builder.xref(number=8,extra=b"/Encrypt 7 0 R /Info 6 0 R /ID [<"+ids+b"><"+ids+b">]")
        if length == 5:
            corrupt=bytearray(data)
            start=data.index(b"\nstream\n",builder.entries[5][1])+len(b"\nstream\n")
            corrupt[start+len(zlib.compress(header+bodies))-1]^=1
            yield Case("security-zero-padded-bad-checksum",bytes(corrupt),INVALID,("security-short-rc4",),
                       "A recognizable decompressed prefix without the complete zlib checksum cannot select a key convention",
                       strict_valid=False,extra={"open_error":"syntax"})
        yield Case(f"security-zero-padded-{length*8}",data,RENDER,("security-short-rc4",),
                   "A full zlib checksum and xref-matching object index establish the nonstandard key convention before strings are cached",
                   oracle=RED,strict_valid=False,repairs=("security.short-key","security.key-padding"),extra={"owner_password":"owner"})


def identifier_syntax_cases():
    for name,reference in [("syntax-reference-object-zeroes",b"02 0 R"),("syntax-reference-generation-zeroes",b"2 00 R")]:
        objects=list(OBJECTS);objects[0]=OBJECTS[0].replace(b"2 0 R",reference)
        yield Case(name,document(objects),RENDER,("identifier-syntax",),
                   "EC3 issue 379 forbids leading zeros in object and generation numbers; numeric identity is preserved with a report",
                   oracle=RED,strict_valid=False,repairs=("syntax.reference-leading-zero",))
    objects=list(OBJECTS);objects[0]=objects[0][:-2]+b" /A{B /C}D >>"
    yield Case("syntax-name-braces",document(objects),RENDER,("identifier-syntax",),
               "Braces are regular name characters outside Type 4 calculator functions (7.2.3)",oracle=RED)


def legacy_selector_cases():
    from .crypto import parameters, encrypted_objects, ID, hex_string
    p,key=parameters(2,b"",b"owner-secret",permissions=-3884)
    objects=encrypted_objects(p,key,"V2")
    oracle=opaque_rectangles(64,64,[((8,8,48,32),(255,0,0)),((0,0,8,8),(0,0,255))])
    yield Case("security-legacy-permission-selector",document(objects,b"/Encrypt 5 0 R /ID ["+hex_string(ID)+b" "+hex_string(ID)+b"] "),RENDER,
               ("security-legacy-permissions",),"Nonconforming R2 selector flags preserve the exact raw permissions in the authenticated hash",
               oracle=oracle,strict_valid=False,repairs=("security.permission-selector",))


def navigation_value(value):
    """Small independent serializer for navigation structures and their oracles."""
    if isinstance(value, tuple):
        kind, item = value
        return (b'/%s' % item.encode()) if kind == 'name' else (b'%d 0 R' % item)
    if value is None:
        return b'null'
    if isinstance(value, bool):
        return b'true' if value else b'false'
    if isinstance(value, bytes):
        return b'<' + value.hex().encode() + b'>'
    if isinstance(value, str):
        return navigation_value(b'\xfe\xff'+value.encode('utf-16be'))
    if isinstance(value, dict):
        return b'<< '+b' '.join(b'/'+key.encode()+b' '+navigation_value(item) for key,item in value.items())+b' >>'
    if isinstance(value, list):
        return b'['+b' '.join(map(navigation_value,value))+b']'
    return str(value).encode()


def navigation_base(pages=2):
    ref = lambda n: ('ref', n)
    name = lambda n: ('name', n)
    return {1: dict(Type=name('Catalog'), Pages=ref(2)),
            2: dict(Type=name('Pages'), Kids=[ref(i+3) for i in range(pages)], Count=pages),
            **{i+3: dict(Type=name('Page'), Parent=ref(2), MediaBox=[0,0,64,64], Resources={}) for i in range(pages)}}


def navigation_vector(identifier, objects, view, expected, *, repair='', **params):
    from .model import Vector
    builder = Builder()
    builder.data = bytearray(HEADER.replace(b'%PDF-1.7',b'%PDF-2.0'))
    for number, value in sorted(objects.items()):
        builder.obj(number, value if isinstance(value, bytes) else navigation_value(value))
    pdf, _ = builder.classic()
    data = dict(view=view, repair=repair, **params)
    return Vector(identifier, {'.pdf':pdf, '.expected':expected.encode(),
        '.params': ''.join(f'{k}={v}\n' for k,v in data.items()).encode()},
        ('navigation-'+view,), 'Independently constructed '+identifier.replace('-',' '))


def navigation_cases():
    import copy
    ref = lambda n: ('ref', n)
    name = lambda n: ('name', n)
    views = [('XYZ',[None,42,0],'XYZ:_,42,_'), ('Fit',[],'Fit'),
             ('FitH',[None],'FitH:_'), ('FitV',[12],'FitV:12'),
             ('FitR',[1,2,30,40],'FitR:1,2,30,40'), ('FitB',[],'FitB'),
             ('FitBH',[7],'FitBH:7'), ('FitBV',[None],'FitBV:_')]
    objects = navigation_base()
    objects[1]['Dests'] = {kind: [ref(3+(i%2)),name(kind),*args] for i,(kind,args,_) in enumerate(views)}
    objects[1]['Names'] = dict(Dests=ref(10))
    objects[10] = dict(Names=[b'alias',ref(11),b'fit-zero',[ref(3),name('XYZ'),1,2,None]])
    objects[11] = dict(D=[ref(4),name('Fit')])
    expected = ''.join(f'{kind}|{i%2}|{label}\n' for i,(kind,args,label) in enumerate(views))
    expected += 'alias|1|Fit\nfit-zero|0|XYZ:1,2,_\n'
    yield navigation_vector('navigation-destinations',objects,'destinations',expected)

    # A broad three-level name tree. Off-path subtrees remain unloaded by a hit.
    objects = navigation_base()
    objects[1]['Names'] = dict(Dests=ref(10))
    objects[10] = dict(Kids=[ref(20+i) for i in range(8)])
    expected=''
    for group in range(8):
        low,high=f'k{group*16:03}',f'k{group*16+15:03}'
        objects[20+group]=dict(Limits=[low.encode(),high.encode()],Kids=[ref(40+group*4+i) for i in range(4)])
        for leaf in range(4):
            start=group*16+leaf*4
            pairs=[]
            for k in range(start,start+4):
                pairs += [f'k{k:03}'.encode(),[ref(3+k%2),name('Fit')]]
                expected += f'k{k:03}|{k%2}|Fit\n'
            objects[40+group*4+leaf]=dict(Limits=[f'k{start:03}'.encode(),f'k{start+3:03}'.encode()],Names=pairs)
    yield navigation_vector('navigation-tree-wide',objects,'trees',expected,query='k073',page=1)
    objects = navigation_base()
    objects[1]['Names']=dict(Dests=ref(10))
    objects[10]=dict(Kids=[ref(11)])
    for i in range(11,31):
        objects[i]=dict(Limits=[b'a',b'a'],Kids=[ref(i+1)])
    objects[31]=dict(Limits=[b'a',b'a'],Names=[b'a',[ref(3),name('Fit')]])
    yield navigation_vector('navigation-tree-deep',objects,'trees','a|0|Fit\n',query='a',page=0)
    for suffix, limits in [('limits-type',42),('limits-missing',None),('limits-range',[b'z',b'z'])]:
        objects=navigation_base()
        objects[1]['Names']=dict(Dests=ref(10))
        objects[10]=dict(Kids=[ref(11)])
        objects[11]=dict(Names=[b'a',[ref(3),name('Fit')]])
        if limits is not None: objects[11]['Limits']=limits
        yield navigation_vector('navigation-tree-'+suffix,objects,'trees','a|0|Fit\n',repair='tree.limits',query='a',page=0)
    objects[11]=dict(Limits=[b'a',b'b'],Names=[b'b',[ref(4),name('Fit')],b'a',[ref(3),name('Fit')],b'a',[ref(4),name('Fit')]])
    yield navigation_vector('navigation-tree-order',objects,'trees','a|0|Fit\nb|1|Fit\n',repair='tree.order',query='a',page=0)
    objects=navigation_base()
    objects[1]['Names']=dict(Dests=ref(10))
    objects[10]=dict(Kids=[ref(11),ref(12)])
    objects[11]=dict(Limits=[b'a',b'a'],Names=[b'a',[ref(3),name('Fit')]])
    objects[12]=dict(Limits=[b'a',b'a'],Kids=[ref(12)])
    yield navigation_vector('navigation-tree-cycle',objects,'trees','a|0|Fit\n',repair='tree.cycle',query='a',page=0)
    objects=navigation_base()
    objects[1]['Names']=dict(Dests=ref(10))
    objects[10]=dict(Kids=[dict(Limits=[b'a',b'a'],Names=[b'a',[ref(3),name('Fit')]])])
    yield navigation_vector('navigation-tree-direct',objects,'trees','a|0|Fit\n',repair='tree.structure',query='a',page=0)

    objects=navigation_base()
    objects[1].update(Outlines=ref(10), Dests=dict(end=[ref(4),name('FitB')]))
    objects[10]=dict(Type=name('Outlines'),First=ref(11),Last=ref(14),Count=3)
    objects[11]=dict(Title=b'Root \xa0',Parent=ref(10),Next=ref(14),First=ref(12),Last=ref(12),Count=1,C=[0.2,0.3,0.4],F=3,Dest=[ref(3),name('Fit')])
    objects[12]=dict(Title='Child 😀',Parent=ref(11),First=ref(13),Last=ref(13),Count=-1,A=dict(S=name('URI'),URI=b'child'))
    objects[13]=dict(Title=b'Hidden',Parent=ref(12),Dest=name('end'))
    objects[14]=dict(Title=b'\xef\xbb\xbf\x1benUS\x1bFin',Parent=ref(10),Prev=ref(11),A=dict(S=name('Named'),N=name('LastPage')))
    expected='Root €|-|0|1|0.2,0.3,0.4|1,1|dest:0:Fit\nChild 😀|0|1|0|0,0,0|0,0|URI:child\nHidden|1|2|0|0,0,0|0,0|dest:1:FitB\nFin|-|0|0|0,0,0|0,0|Named:LastPage\n'
    yield navigation_vector('navigation-outlines',objects,'outlines',expected)
    optional=copy.deepcopy(objects);optional[99]=None
    optional[10]['Type']=ref(99)
    optional[11]['Prev']=ref(99)
    optional[12]['Next']=ref(99)
    optional[14].update(F=ref(99),C=ref(99),First=ref(99),Last=ref(99),Next=ref(99),Dest=ref(99))
    yield navigation_vector('navigation-outlines-optional-null',optional,'outlines',expected)
    whitespace=copy.deepcopy(objects);whitespace[11]['Title']=b'Root\t\xa0\n\r'
    yield navigation_vector('navigation-outlines-pdfdoc-whitespace',whitespace,'outlines',expected.replace('Root €|','Root\t€\n\r|'))
    for suffix,change,repair in [
        ('cycle',lambda o:o[14].update(Next=ref(11)),'outline.cycle'),
        ('links',lambda o:o[14].update(Prev=ref(13)),'outline.links'),
        ('parent',lambda o:o[12].update(Parent=ref(10)),'outline.links'),
        ('last',lambda o:o[10].update(Last=ref(13)),'outline.links'),
        ('dangling',lambda o:o[14].update(Prev=ref(99)),'outline.links'),
        ('count',lambda o:o[11].update(Count=99),'outline.count'),
        ('type',lambda o:o[10].update(Type=name('Wrong')),'outline.type'),
    ]:
        damaged=copy.deepcopy(objects);change(damaged)
        yield navigation_vector('navigation-outlines-'+suffix,damaged,'outlines',expected,repair=repair)
    damaged=copy.deepcopy(objects);damaged[14]['Title']=b'\xfe\xff\xd8\x00'
    yield navigation_vector('navigation-outlines-text',damaged,'outlines',expected.replace('Fin|','�|'),repair='string.invalid-text')

    objects=navigation_base()
    objects[1]['URI']=dict(Base=b'https://example.invalid/docs/')
    kinds=[dict(S=name('GoTo'),D=[ref(4),name('Fit')]),
           dict(S=name('GoToR'),F=dict(Type=name('Filespec'),UF='remote.pdf'),D=[77,name('XYZ'),None,None,0],NewWindow=True),
           dict(S=name('GoToR'),F=b'remote.pdf',D=b'remote'),
           dict(S=name('URI'),URI=b'../chapter',IsMap=True),
           *[dict(S=name('Named'),N=name(n)) for n in ['NextPage','PrevPage','FirstPage','LastPage']],
           dict(S=name('Launch'),F=b'inert.bin',Win=dict(F=b'inert.exe')),
           dict(S=name('JavaScript'),JS=b'never execute'),
           dict(S=name('JavaScript'),JS=ref(40))]
    objects[40]=stream(b'also never execute')
    labels=['GoTo:1:Fit','GoToR:77:XYZ:_,_,_:true','GoToR:name:remote','URI:../chapter','Named:NextPage','Named:PrevPage','Named:FirstPage','Named:LastPage','Launch:inert.bin','JavaScript:string','JavaScript:stream']
    objects[3]['Annots']=[ref(10+i) for i in range(len(kinds))]
    expected=''
    for i,(action,label) in enumerate(zip(kinds,labels)):
        objects[10+i]=dict(Type=name('Annot'),Subtype=name('Link'),Rect=[1,2,31,42],QuadPoints=[1,2,31,2,31,42,1,42],H=name(['N','I','O','P'][i%4]),BS=dict(W=2,S=name('D'),D=[3,1]),A=action)
        expected+=f'{i}|1,2,31,42|1|2|{i%4}|{label}\n'
    # Next arrays are preorder, and reuse is not mistaken for a cycle.
    objects[13]['A']['Next']=[dict(S=name('GoTo'),D=[ref(3),name('Fit')],Next=ref(41)),ref(41)]
    objects[41]=dict(S=name('Named'),N=name('LastPage'))
    expected=expected.replace('3|1,2,31,42|1|2|3|URI:../chapter\n','3|1,2,31,42|1|2|3|URI:../chapter,GoTo:0:Fit,Named:LastPage,Named:LastPage\n')
    yield navigation_vector('navigation-links-actions',objects,'links',expected)
    damaged=copy.deepcopy(objects);damaged[10]['Dest']=[ref(3),name('Fit')]
    yield navigation_vector('navigation-links-target-conflict',damaged,'links',expected,repair='navigation.target-conflict')
    damaged=copy.deepcopy(objects);damaged[10]['QuadPoints']=[1,2,3]
    yield navigation_vector('navigation-links-quads',damaged,'links',expected.replace('0|1,2,31,42|1|','0|1,2,31,42|0|',1),repair='link.quad-points')
    objects=navigation_base();objects[3]['Annots']=[ref(10)]
    objects[10]=dict(Subtype=name('Link'),Rect=[1,2,31,42],A=ref(11))
    objects[11]=dict(S=name('Named'),N=name('NextPage'),Next=ref(11))
    yield navigation_vector('navigation-action-cycle',objects,'links','0|1,2,31,42|0|1|1|Named:NextPage\n',repair='action.cycle')

    objects=navigation_base(10)
    styles=[(0,dict(P=b'Intro')),(1,dict(S=name('D'),St=8,P=b'A-')),(3,dict(S=name('R'),St=9)),(4,dict(S=name('r'),St=4)),(5,dict(S=name('A'),St=26)),(6,dict(S=name('A'),St=27)),(7,dict(S=name('a'),St=52)),(8,dict(S=name('a'),St=53)),(9,dict(S=name('D'),P='章-'))]
    objects[1]['PageLabels']=dict(Kids=[ref(20),ref(21)])
    objects[20]=dict(Limits=[0,4],Nums=[v for pair in styles[:4] for v in pair])
    objects[21]=dict(Limits=[5,9],Nums=[v for pair in styles[4:] for v in pair])
    yield navigation_vector('navigation-page-labels',objects,'labels','Intro\nA-8\nA-9\nIX\niv\nZ\nAA\nzz\naaa\n章-1\n')

    objects=navigation_base()
    objects[1]['Dests']=dict(zero=[0,name('Fit')],one=[1,name('XYZ'),None,42,0])
    yield navigation_vector('navigation-destination-page-number',objects,'destinations','zero|0|Fit\none|1|XYZ:_,42,_\n',repair='destination.page-number')
    objects=navigation_base()
    objects[1]['Names']=dict(Dests=ref(10))
    objects[10]=dict(Names=[b'a',[ref(3),name('Fit')]],Kids=[ref(11)])
    objects[11]=dict(Limits=[b'b',b'b'],Names=[b'b',[ref(4),name('Fit')]])
    yield navigation_vector('navigation-tree-mixed',objects,'trees','a|0|Fit\nb|1|Fit\n',repair='tree.structure',query='a',page=0)
    for suffix,view,change,error in [
        ('tree-unmatched','trees',lambda o:(o[1].update(Names=dict(Dests=ref(10))),o.update({10:dict(Names=[b'bad'])})),'syntax'),
        ('destination-cycle','destinations',lambda o:o[1].update(Dests=dict(cycle=dict(D=name('cycle')))),'cycle'),
        ('labels-overflow','labels',lambda o:o[1].update(PageLabels=dict(Nums=[0,dict(S=name('A'),St=9223372036854775807)])),'limit'),
        ('link-highlight-type','links',lambda o:(o[3].update(Annots=[ref(10)]),o.update({10:dict(Subtype=name('Link'),Rect=[0,0,1,1],H=42)})),'syntax'),
    ]:
        objects=navigation_base();change(objects)
        yield navigation_vector('navigation-'+suffix,objects,view,'',error=error)

    objects=navigation_base()
    objects[3]['Annots']=[ref(10+i) for i in range(5)]
    expected=''
    for i,style in enumerate(['S','D','B','I','U']):
        objects[10+i]=dict(Subtype=name('Link'),Rect=[31,42,1,2],H=name('I'),Border=42,
            BS=dict(S=name(style),W=i+1),Dest=[ref(3),name('Fit')],QuadPoints=[0,0,31,2,31,42,1,42])
        expected+=f'{i}|1,2,31,42|0|{i+1}|1|dest:0:Fit\n'
    yield navigation_vector('navigation-links-border-styles',objects,'links',expected,
        border_styles='Solid,Dashed,Beveled,Inset,Underline')
    objects=navigation_base()
    objects[3]['Annots']=[ref(10)]
    objects[10]=dict(Subtype=name('Link'),Rect=[1,2,31,42],Border=[4,5,2,[3,1]],Dest=[ref(3),name('Fit')])
    yield navigation_vector('navigation-link-border-array',objects,'links','0|1,2,31,42|0|2|1|dest:0:Fit\n',border_styles='Dashed')
    objects=navigation_base()
    objects[1]['Names']=dict(Dests=ref(10))
    objects[10]=dict(Kids=[ref(99),ref(11)])
    objects[11]=dict(Limits=[b'a',b'a'],Names=[b'a',[ref(3),name('Fit')]])
    yield navigation_vector('navigation-tree-dangling-child',objects,'trees','a|0|Fit\n',repair='tree.structure',query='a',page=0)
    objects[10]['Kids']=[ref(11)]
    objects[11]['Limits']=ref(99)
    yield navigation_vector('navigation-tree-dangling-limits',objects,'trees','a|0|Fit\n',repair='tree.limits',query='a',page=0)

    objects=navigation_base();objects[99]=None
    objects[1].update(**{key:ref(99) for key in ['Outlines','Dests','Names','PageLabels','URI']})
    objects[3]['Annots']=[ref(10+i) for i in range(5)]
    actions=[dict(A=ref(99),Dest=ref(99)),dict(A=dict(S=name('URI'),URI=b'relative',Next=ref(99))),dict(A=ref(99),Dest=[ref(4),name('Fit')]),dict(A=dict(S=name('URI'),URI=b'relative'),Dest=ref(99)),dict(A=dict(S=name('Launch'),F=ref(99),Win=dict(F=b'inert'),NewWindow=ref(99)))]
    expected=''
    for i,(target,label) in enumerate(zip(actions,['none','URI:relative','dest:1:Fit','URI:relative','Launch:platform'])):
        objects[10+i]=dict(Subtype=name('Link'),Rect=[1,2,31,42],BS=ref(99),Border=ref(99),H=ref(99),QuadPoints=ref(99),**target)
        expected+=f'{i}|1,2,31,42|0|1|1|{label}\n'
    yield navigation_vector('navigation-optional-null',objects,'links',expected)
    undefined=copy.deepcopy(objects);del undefined[99]
    yield navigation_vector('navigation-optional-undefined',undefined,'links',expected)
    objects=navigation_base();objects[99]=None
    objects[1].update(Dests=dict(absent=ref(99),also_absent=ref(88)),Names=dict(Dests=ref(99)))
    yield navigation_vector('navigation-named-optional-null',objects,'destinations','')

    objects=navigation_base();objects[99]=None
    objects[3]['Annots']=[ref(10)]
    objects[10]=dict(Subtype=name('Link'),Rect=[1,2,31,42],A=dict(S=name('Launch'),Win=ref(99)))
    yield navigation_vector('navigation-launch-null-platform',objects,'links','',error='syntax')
