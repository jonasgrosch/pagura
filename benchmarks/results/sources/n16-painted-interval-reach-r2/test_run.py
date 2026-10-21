"""Pure parser/oracle tests: no subprocess, fixture generation or rendering."""
import importlib.util
from pathlib import Path
import sys
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).parent))
SPEC = importlib.util.spec_from_file_location("reach_run", Path(__file__).with_name("run.py"))
probe = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(probe)


def request(name):
    return next(value for value in probe.REQUESTS if value[0] == name)


def log_for(name, mode, chunks):
    dimension = request(name)[2]
    lines = [f"plan mode={mode.title()} competitive={str(mode == 'competitive').lower()} "
             f"format=N16 side=256 tile=0,0,{dimension},{dimension} background=255,255,255,255"]
    for tile, rows, output in chunks:
        x, y, width, height = tile
        lines += [f"chunk tile={x},{y},{width},{height} format=N16",
                  f"surface format=N16 bytes={width * height * 8}"]
        lines += [f"writer route={route} offset={offset} count={count} nonzero={positive}"
                  for route, offset, count, positive in rows]
        start, end, pixels, calls = output
        lines += [f"output storage=N16 start={start} end={end} pixels={pixels} calls={calls} total={width * height}"]
    return "\n".join(probe.MARKER + line for line in lines)


def check(name, mode, log):
    return probe.trace(log, request(name), mode, [255] * 4)


class ReachTests(unittest.TestCase):
    def tiny(self):
        rows = [("path", y * 64 + 24, 8, 8) for y in range(32, 40)]
        return log_for("opaque-tiny", "exact", [((0, 0, 64, 64), rows, (2072, 2528, 456, 8))])

    def test_conservative_tiny_range_and_calls(self):
        _, chunks, count = check("opaque-tiny", "exact", self.tiny())
        self.assertEqual(chunks[0]["writers"], {"path": 8, "fragment": 0})
        self.assertEqual(chunks[0]["output"]["pixels"], "456")
        self.assertEqual((chunks[0]["coverage"]["nonzero"], count), (64, 12))

    def test_rejects_ink_count_wrong_writer_retry_and_missing_output(self):
        log = self.tiny()
        bad = (log.replace("pixels=456", "pixels=64"), log.replace("route=path", "route=fragment"),
               log + "\n" + log.splitlines()[0], "\n".join(log.splitlines()[:-1]),
               log.replace("offset=2072 count=8", "offset=2072 count=64"),
               log.replace("nonzero=8", "nonzero=9"))
        for value in bad:
            with self.subTest(value=value), self.assertRaises(ValueError):
                check("opaque-tiny", "exact", value)

    def test_empty_constructor_interval_and_partition(self):
        chunks = [((x, y, 256, 256), [], (65536, 0, 0, 0)) for y in (0, 256) for x in (0, 256)]
        log = log_for("opaque-empty", "competitive", chunks)
        _, parsed, count = check("opaque-empty", "competitive", log)
        self.assertEqual((len(parsed), count), (4, 13))
        with self.assertRaises(ValueError):
            check("opaque-empty", "competitive", log.replace("tile=256,0,256,256", "tile=0,0,256,256"))

    def test_fresh_position_requires_observed_zero_halo(self):
        rows = [("fragment", 60684, 8, 4), ("fragment", 60940, 8, 4), ("fragment", 61196, 8, 0)]
        log = log_for("fresh-h2-position", "competitive", [((0, 0, 256, 256), rows, (60684, 61204, 520, 3))])
        _, chunks, _ = check("fresh-h2-position", "competitive", log)
        self.assertEqual(chunks[0]["coverage"], {"submitted": 24, "nonzero": 8,
                                               "all_zero_writers": 1, "halo_rows": {"239": 0}})
        # Keep total positive count8, but move one positive sample into the halo.
        bad = log.replace("offset=60940 count=8 nonzero=4", "offset=60940 count=8 nonzero=3")
        bad = bad.replace("offset=61196 count=8 nonzero=0", "offset=61196 count=8 nonzero=1")
        with self.assertRaisesRegex(ValueError, "halo"):
            check("fresh-h2-position", "competitive", bad)

    def test_fresh_right_clipped_stride(self):
        rows = [("fragment", y * 256 + 252, 4, 4 if y < 15 else 0) for y in range(10, 16)]
        log = log_for("fresh-h5-right-clip", "competitive", [((0, 0, 256, 256), rows, (2812, 4096, 1284, 6))])
        _, chunks, _ = check("fresh-h5-right-clip", "competitive", log)
        self.assertEqual(chunks[0]["coverage"]["submitted"], 24)
        with self.assertRaises(ValueError):
            check("fresh-h5-right-clip", "competitive", log.replace("count=4", "count=8"))

    def test_fresh_four_chunk_crossing_both_modes(self):
        # Literal submitted rectangles in local coordinates, independent of the plan.
        for mode, right, bottom, intervals in (
            ("exact", 2, 4, ((64510, 65536, 1026, 5), (64256, 65282, 1026, 5),
                              (254, 1024, 770, 4), (0, 770, 770, 4))),
            ("competitive", 6, 5, ((64510, 65536, 1026, 5), (64256, 65286, 1030, 5),
                                    (254, 1280, 1026, 5), (0, 1030, 1030, 5))),
        ):
            route = "path" if mode == "exact" else "fragment"
            chunks = []
            for index, (x, y) in enumerate(((0, 0), (256, 0), (0, 256), (256, 256))):
                first, last = (251, 256) if y == 0 else (0, bottom)
                left, count = (254, 2) if x == 0 else (0, right)
                rows = [(route, row * 256 + left, count, 2 if y == 0 or row < 4 else 0)
                        for row in range(first, last)]
                chunks.append(((x, y, 256, 256), rows, intervals[index]))
            _, parsed, _ = check("fresh-h9-chunk-crossing", mode, log_for("fresh-h9-chunk-crossing", mode, chunks))
            self.assertEqual([chunk["coverage"]["nonzero"] for chunk in parsed], [10, 10, 8, 8])

    def test_r2_opaque64_padding_rejects_old_six_row_prediction(self):
        rows = [("fragment", row * 256 + x, min(8, 256 - x), 4 if row < 240 else 0)
                for x in range(0, 256, 4) for row in range(234, 241)]
        chunks = [((x, y, 256, 256), rows, (59904, 61696, 1792, 7))
                  for y in range(0, 2048, 256) for x in range(0, 2048, 256)]
        log = log_for("opaque64", "competitive", chunks)
        _, parsed, _ = check("opaque64", "competitive", log)
        self.assertEqual(len(parsed), 64)
        with self.assertRaises(ValueError):
            check("opaque64", "competitive", log.replace("end=61696 pixels=1792 calls=7", "end=61440 pixels=1536 calls=6"))

    def test_rgba_alpha_and_background_are_not_normalized(self):
        header = b"RGBA8 1 1 premultiplied\n"
        probe.verify_output(header + bytes((191, 191, 191, 255)), bytes((191,)) * 3, 1, False)
        probe.verify_output(header + bytes((9, 43, 85, 128)), b"\xff" * 3, 1, True)
        for pixel, nonopaque in ((bytes((191, 191, 191, 254)), False),
                                 (bytes((17, 85, 170, 128)), True)):
            with self.assertRaises(ValueError):
                probe.verify_output(header + pixel, bytes((191,)) * 3, 1, nonopaque)

    def test_fixed_plan_is_post_discovery_24_request_matrix(self):
        plan = probe.frozen_plan()
        self.assertEqual((plan["revision"], plan["expected_outputs"], len(plan["requests"])), ("R2", 24, 24))
        self.assertTrue(plan["post_discovery_revision"])
        self.assertEqual(len({(row["request"], row["mode"]) for row in plan["requests"]}), 24)


if __name__ == "__main__":
    unittest.main()
