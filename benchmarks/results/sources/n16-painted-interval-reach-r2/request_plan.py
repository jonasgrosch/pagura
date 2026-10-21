"""R2 fixed native requests, frozen before the three new inputs are rendered.

R1's failed six-row Competitive hypothesis remains failed. Only opaque64's
Competitive interval is revised from source padding proof; the last three
requests are new controls. These counts are reach hypotheses, never timings.
"""

REVISION = "R2"
EXPECTED_OUTPUTS = 24
# request, fixture, dimension, storage, chunk side, common N16 output tuple
REQUESTS = (
    ("opaque64", "opaque64", 2048, "N16", 256, (59904, 61440, 1536, 6)),
    ("opaque-empty", "opaque-empty", 512, "N16", 256, (65536, 0, 0, 0)),
    ("opaque-distant", "opaque-distant", 512, "N16", 256, (0, 65536, 65536, 256)),
    ("opaque-fill", "opaque-fill", 512, "N16", 256, (0, 65536, 65536, 256)),
    ("opaque-tiny", "opaque-tiny", 64, "N16", 256, (2072, 2528, 456, 8)),
    ("sparse64", "sparse64", 2048, "F32", 256, None),
    ("default-knockout64", "default-knockout64", 2048, "F64", 256, None),
    ("image-only", "image-only", 512, "F32", 128, None),
    ("nonopaque-empty", "opaque-empty", 512, "F64", 256, None),
    ("fresh-h2-position", "fresh-h2-position", 256, "N16", 256, None),
    ("fresh-h5-right-clip", "fresh-h5-right-clip", 256, "N16", 256, None),
    ("fresh-h9-chunk-crossing", "fresh-h9-chunk-crossing", 512, "N16", 256, None),
)

# For each chunk: interval tuple, submitted samples, positive samples,
# writer calls, all-zero writer calls, local rows requiring observed zero halo.
FRESH = {
    ("fresh-h2-position", "exact"): (((60684, 60944, 260, 2), 8, 8, 2, 0, ()),),
    ("fresh-h2-position", "competitive"): (((60684, 61204, 520, 3), 24, 8, 3, 1, (239,)),),
    ("fresh-h5-right-clip", "exact"): (((2812, 3840, 1028, 5), 20, 20, 5, 0, ()),),
    ("fresh-h5-right-clip", "competitive"): (((2812, 4096, 1284, 6), 24, 20, 6, 1, (15,)),),
    ("fresh-h9-chunk-crossing", "exact"): (
        ((64510, 65536, 1026, 5), 10, 10, 5, 0, ()),
        ((64256, 65282, 1026, 5), 10, 10, 5, 0, ()),
        ((254, 1024, 770, 4), 8, 8, 4, 0, ()),
        ((0, 770, 770, 4), 8, 8, 4, 0, ()),
    ),
    ("fresh-h9-chunk-crossing", "competitive"): (
        ((64510, 65536, 1026, 5), 10, 10, 5, 0, ()),
        ((64256, 65286, 1030, 5), 30, 10, 5, 0, ()),
        ((254, 1280, 1026, 5), 10, 8, 5, 1, (4,)),
        ((0, 1030, 1030, 5), 30, 8, 5, 1, (4,)),
    ),
}


def expected_n16(request, mode, chunk_index):
    name = request[0]
    if (name, mode) in FRESH:
        interval, submitted, nonzero, calls, zero_calls, halo = FRESH[name, mode][chunk_index]
        return {"interval": interval, "submitted": submitted, "nonzero": nonzero,
                "writer_calls": calls, "all_zero_writers": zero_calls, "halo_rows": halo}
    positive = {"opaque64": 1536, "opaque-empty": 0, "opaque-distant": 2,
                "opaque-fill": 65536, "opaque-tiny": 64}[name]
    competitive_glyph = name == "opaque64" and mode == "competitive"
    return {"interval": (59904, 61696, 1792, 7) if competitive_glyph else request[5],
            "nonzero": positive, "halo_rows": (240,) if competitive_glyph else ()}


def writer_route(name, mode):
    glyph = name == "opaque64" or name.startswith("fresh-")
    return "fragment" if glyph and mode == "competitive" else "path"


def frozen_plan():
    return {"schema": 2, "revision": REVISION, "expected_outputs": EXPECTED_OUTPUTS,
            "post_discovery_revision": True,
            "r1_status": "failed reach hypothesis; retained unchanged",
            "requests": [
                {"request": request[0], "fixture": request[1], "dimension": request[2],
                 "storage": request[3], "side": request[4], "mode": mode,
                 "operation": "full", "dpi": 72,
                 "n16_chunks": [expected_n16(request, mode, index)
                                for index in range(((request[2] + request[4] - 1) // request[4]) ** 2)]
                 if request[3] == "N16" else []}
                for request in REQUESTS for mode in ("exact", "competitive")]}
