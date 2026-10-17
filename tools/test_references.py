"""Acquisition metadata must preserve user-supplied material and detect drift."""
import hashlib
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import references


class ReferenceTests(unittest.TestCase):
    def test_refresh_preserves_registered_full_text(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            private = root / "references/private/core.pdf"
            private.parent.mkdir(parents=True)
            private.write_bytes(b"%PDF-test")
            registered = {"id": "core", "status": "locally-supplied", "cache_path": "references/private/core.pdf",
                          "sha256": hashlib.sha256(private.read_bytes()).hexdigest()}
            with patch.object(references, "ROOT", root):
                merged = references.preserve_local({"sources": [registered]}, [{"id": "core", "status": "requires-user-acquisition"}])
                self.assertEqual(merged, [registered])
                private.write_bytes(b"modified")
                with self.assertRaises(ValueError):
                    references.preserve_local({"sources": [registered]}, [])


if __name__ == "__main__":
    unittest.main()
