"""Pure parser/oracle tests; never invoke a renderer or fixture generator."""
import importlib.util
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location("reach_run", Path(__file__).with_name("run.py"))
probe = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(probe)


class ReachTests(unittest.TestCase):
    def tiny(self):
        lines = ["plan mode=Exact competitive=false format=N16 side=256 tile=0,0,64,64 background=255,255,255,255",
                 "chunk tile=0,0,64,64 format=N16", "surface format=N16 bytes=32768"]
        lines += [f"writer route=path offset={y * 64 + 24} count=8" for y in range(32, 40)]
        lines += ["output storage=N16 start=2072 end=2528 pixels=456 calls=8 total=4096"]
        return "\n".join(probe.MARKER + line for line in lines)

    def check_tiny(self, log):
        return probe.trace(log, probe.REQUESTS[4], "exact", [255] * 4)

    def test_conservative_tiny_range_and_calls(self):
        _, chunks, count = self.check_tiny(self.tiny())
        self.assertEqual(chunks[0]["writers"], {"path": 8, "fragment": 0})
        self.assertEqual(chunks[0]["output"]["pixels"], "456")
        self.assertEqual(count, 12)

    def test_rejects_ink_count_wrong_writer_retry_and_missing_output(self):
        log = self.tiny()
        bad = (log.replace("pixels=456", "pixels=64"), log.replace("route=path", "route=fragment"),
               log + "\n" + log.splitlines()[0], "\n".join(log.splitlines()[:-1]),
               log.replace("offset=2072 count=8", "offset=2072 count=64"))
        for value in bad:
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.check_tiny(value)

    def test_empty_constructor_interval_and_partition(self):
        lines = ["plan mode=Competitive competitive=true format=N16 side=256 tile=0,0,512,512 background=255,255,255,255"]
        for y in (0, 256):
            for x in (0, 256):
                lines += [f"chunk tile={x},{y},256,256 format=N16", "surface format=N16 bytes=524288",
                          "output storage=N16 start=65536 end=0 pixels=0 calls=0 total=65536"]
        log = "\n".join(probe.MARKER + line for line in lines)
        _, chunks, count = probe.trace(log, probe.REQUESTS[1], "competitive", [255] * 4)
        self.assertEqual((len(chunks), count), (4, 13))
        with self.assertRaises(ValueError):
            probe.trace(log.replace("tile=256,0,256,256", "tile=0,0,256,256"),
                        probe.REQUESTS[1], "competitive", [255] * 4)

    def test_rgba_alpha_and_background_are_not_normalized(self):
        header = b"RGBA8 1 1 premultiplied\n"
        probe.verify_output(header + bytes((191, 191, 191, 255)), bytes((191,)) * 3, 1, False)
        probe.verify_output(header + bytes((9, 43, 85, 128)), b"\xff" * 3, 1, True)
        for pixel, nonopaque in ((bytes((191, 191, 191, 254)), False),
                                 (bytes((17, 85, 170, 128)), True)):
            with self.assertRaises(ValueError):
                probe.verify_output(header + pixel, bytes((191,)) * 3, 1, nonopaque)


if __name__ == "__main__":
    unittest.main()
