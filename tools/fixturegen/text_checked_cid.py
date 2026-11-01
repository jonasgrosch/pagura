"""Independent code/Unicode/glyph tuples for checked Latin CID substitution.

Only the locked sfnt cmap supplies expected glyph IDs; no product is invoked.
Minion to Times is an explicit visual fallback, not metric compatibility.
"""
from .model import Case, RENDER, Vector


def cid_document(ordering, cmap, *, extra=b"", descendant=b"", subtype=b"CIDFontType2", unicode_map=None, base=b"MinionPro-Regular"):
    from .pdf import stream
    from .text import pdf

    font = b"<< /Type /Font /Subtype /Type0 /BaseFont /"+base+b" /DescendantFonts [5 0 R] /Encoding "+cmap
    font += (b" /ToUnicode 8 0 R" if unicode_map else b"")+b" >>"
    cidfont = b"<< /Type /Font /Subtype /"+subtype+b" /BaseFont /"+base+b" /CIDSystemInfo << /Registry (Adobe) /Ordering ("+ordering.encode()+b") /Supplement 0 >> /FontDescriptor 6 0 R "+descendant+b" >>"
    descriptor = b"<< /Type /FontDescriptor /FontName /"+base+b" /Flags 6 /FontBBox [-200 -300 1200 1000] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 >>"
    return pdf(font, [cidfont, descriptor, stream(extra) if extra else b"null", stream(unicode_map) if unicode_map else b"null"])


def cases(data, lock):
    from .text import bundled_tt_glyph, expected, pdf

    requirement = ("text-checked-cid-substitution",)
    encoding = (b"1 begincodespacerange <0000> <ffff> endcodespacerange "
                b"3 begincidchar <0101> 29 <0202> 73 <0303> 11 endcidchar")
    unicode = (b"1 begincodespacerange <0000> <ffff> endcodespacerange "
               b"3 beginbfchar <0101> <0041> <0202> <0020> <0303> <0057> endbfchar")
    widths = b"/DW 877 /W [29 [731] 73 [423]]"
    codes = b"\x01\x01\x02\x02\x03\x03"
    for suffix, face in [("Regular", "Times-Roman"), ("Bold", "Times-Bold"),
                         ("Italic", "Times-Italic"), ("BoldItalic", "Times-BoldItalic")]:
        outline = data/lock["standard_14"][face]["outline"]
        rows = [(source, cid, width, chr(scalar), bundled_tt_glyph(outline, scalar))
                for source, cid, width, scalar in [(0x101,29,731,65), (0x202,73,423,32), (0x303,11,877,87)]]
        assert all(row[4] for row in rows)
        for subtype in (b"CIDFontType0", b"CIDFontType2"):
            name = "checked-cid-"+suffix.lower()+"-"+subtype.decode()
            yield Vector(name, {".pdf":cid_document("Identity", b"7 0 R", extra=encoding,
                         descendant=widths, unicode_map=unicode, base=b"ABCDEF+MinionPro-"+suffix.encode(), subtype=subtype),
                         ".input":codes, ".out":expected(rows), ".base":face.encode()}, requirement,
                         "Distinct source codes, CIDs and Unicode select locked substitute glyphs; PDF W/DW retain advances, including a valid space",
                         {"driver":"checked-cid"})
    yield from paint_cases(data, lock)
    # Identity-H and an absent CIDSystemInfo are separate existing routing cases.
    outline = data/lock["standard_14"]["Times-Roman"]["outline"]
    for name, ordering in [("identity", "Identity"), ("unnamed", "Identity")]:
        content = cid_document(ordering, b"/Identity-H", descendant=b"/DW 877 /W [257 [731]]",
                                          unicode_map=unicode, base=b"MinionPro-Regular")
        if name == "unnamed":
            # Rebuild the authored object graph; never mutate serialized bytes/xref.
            from .pdf import stream
            content = pdf(b"<< /Subtype /Type0 /BaseFont /MinionPro-Regular /Encoding /Identity-H /DescendantFonts [5 0 R] /ToUnicode 8 0 R >>",
                          [b"<< /Subtype /CIDFontType0 /BaseFont /MinionPro-Regular /DW 877 /W [257 [731]] /FontDescriptor 6 0 R >>",
                           b"<< /Flags 6 /FontName /MinionPro-Regular >>", b"null", stream(unicode)])
        rows = [(0x101,0x101,731,"A",bundled_tt_glyph(outline,65))]
        yield Vector("checked-cid-"+name, {".pdf":content, ".input":codes[:2], ".out":expected(rows), ".base":b"Times-Roman"},
                     requirement, "Declared source Unicode remains authoritative through Identity-H, with or without a named collection", {"driver":"checked-cid"})
    faults = [
        ("no-unicode", None, encoding, b"MinionPro-Regular", b"7 0 R", codes),
        ("cid-is-not-unicode", b"/CMapType 1 def "+encoding, encoding, b"MinionPro-Regular", b"7 0 R", codes),
        ("unmapped-unicode", unicode.replace(b"<0303> <0057>", b"<0404> <0057>"), encoding, b"MinionPro-Regular", b"7 0 R", codes),
        ("ligature", unicode.replace(b"<0057>", b"<00660069>"), encoding, b"MinionPro-Regular", b"7 0 R", codes),
        ("empty-unicode", unicode.replace(b"<0057>", b"<>"), encoding, b"MinionPro-Regular", b"7 0 R", codes),
        ("missing-glyph", unicode.replace(b"<0057>", b"<DBFFDFFF>"), encoding, b"MinionPro-Regular", b"7 0 R", codes),
        ("encoding-hole", unicode, encoding.replace(b"<0303> 11", b"<0404> 11"), b"MinionPro-Regular", b"7 0 R", codes),
        ("notdef-cid", unicode, encoding.replace(b"<0303> 11", b"<0303> 0"), b"MinionPro-Regular", b"7 0 R", codes),
        ("unknown-style", unicode, encoding, b"MinionPro-Black", b"7 0 R", codes),
        ("unknown-family", unicode, encoding, b"UnknownSerif-Regular", b"7 0 R", codes),
        ("invalid-subset", unicode, encoding, b"ABCDE+MinionPro-Regular", b"7 0 R", codes),
        ("punctuation-name", unicode, encoding, b"Minion--Pro-Regular", b"7 0 R", codes),
        ("case-name", unicode, encoding, b"MINIONPRO-Regular", b"7 0 R", codes),
        ("vertical", unicode, encoding, b"MinionPro-Regular", b"/Identity-V", codes),
    ]
    for name, mapping, cmap, base, cmap_ref, input in faults:
        tag = b"font.cid-substitute-vertical" if name == "vertical" else b"font.non-embedded"
        files = {".pdf":cid_document("Identity", cmap_ref, extra=cmap,
                 descendant=widths, unicode_map=mapping, base=base), ".input":input, ".error":tag+b"\n"}
        if name == "cid-is-not-unicode":
            files['.strict.error'] = b'font.tounicode-invalid\n'
        yield Vector("checked-cid-"+name, files, requirement,
                     "An incomplete or unknown font never yields a successful blank substitute",
                     {"driver":"checked-cid-malformed" if name == "cid-is-not-unicode" else "checked-cid-error"})
    yield Vector("checked-cid-simple-unchanged", {".pdf":pdf(b"<< /Subtype /Type1 /BaseFont /MinionPro-Regular >>"),
                 ".input":b"A", ".error":b"font.non-embedded\n"}, requirement,
                 "CID-only policy does not enable simple-font aliases or invent missing metrics", {"driver":"checked-cid-error"})
    yield Vector("checked-cid-malformed-unicode", {".pdf":cid_document("Identity", b"7 0 R", extra=encoding,
                 descendant=widths, unicode_map=unicode.replace(b"<0057>", b"<D800>"), base=b"MinionPro-Regular"),
                 ".input":codes, ".strict.error":b"font.tounicode-utf16\n", ".error":b"font.non-embedded\n"},
                 requirement, "Malformed Unicode retains the existing Strict repair refusal; Repair still cannot invent the used glyph identity",
                 {"driver":"checked-cid-malformed"})


def paint_cases(data, lock):
    """Exact rational area of independently read straight-sided Tinos glyphs."""
    from .paint import bundled_oracle, page
    from .pdf import stream

    encoding = (b"1 begincodespacerange <0000> <ffff> endcodespacerange "
                b"2 begincidchar <0101> 29 <0303> 11 endcidchar")
    unicode = (b"1 begincodespacerange <0000> <ffff> endcodespacerange "
               b"2 beginbfchar <0101> <0048> <0303> <0048> endbfchar")
    for suffix, face in [("Regular", "Times-Roman"), ("Bold", "Times-Bold"),
                         ("Italic", "Times-Italic"), ("BoldItalic", "Times-BoldItalic")]:
        item = lock['standard_14'][face]
        oracle = bundled_oracle(data/item['outline'], data/item['metrics'], 'HH', advance=1000)
        for subtype in (b"CIDFontType0", b"CIDFontType2"):
            for route, base, information in [
                ('checked', b"ABCDEF+MinionPro-"+suffix.encode(), ('font.substituted',)),
                ('legacy', face.encode(), ('font.cid-glyph-missing', 'font.substituted')),
            ]:
                objects = [b"<< /Type /Font /Subtype /Type0 /BaseFont /"+base+b" /DescendantFonts [6 0 R] /Encoding 8 0 R /ToUnicode 9 0 R >>",
                           b"<< /Type /Font /Subtype /"+subtype+b" /BaseFont /"+base+b" /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 7 0 R /DW 877 /W [29 [1000]] >>",
                           b"<< /Type /FontDescriptor /FontName /"+base+b" /Flags 6 /FontBBox [-200 -300 1200 1000] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 >>",
                           stream(encoding), stream(unicode)]
                yield Case('paint-'+route+'-cid-'+suffix.lower()+'-'+subtype.decode(),
                           page(b'BT /F 24 Tf 3 8 Td <01010303> Tj ET', objects, b'<< /Font << /F 5 0 R >> >>', 64),
                           RENDER, ('text-checked-cid-substitution',),
                           'Source code to Unicode to locked '+face+' contours, exact rational pixel areas and PDF W/DW placement; visual fallback only',
                           oracle=oracle, information=information, extra={'tolerance':1})
