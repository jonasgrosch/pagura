"""Public artifact boundaries and required license distribution."""
import tempfile
import unittest
from pathlib import Path

from package_pages import package


class PackagePagesTests(unittest.TestCase):
    def test_public_assets_only(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            wasm = root / "module.wasm"
            wasm.write_bytes(b"\0asm\x01\0\0\0")
            output = root / "site"
            package(output, wasm)
            self.assertTrue((output / "start.mjs").is_file())
            self.assertTrue((output / "isolation_worker.mjs").is_file())
            self.assertEqual((output / "pagura-host.wasm").read_bytes(), wasm.read_bytes())
            self.assertTrue((output / "licenses/data/afm/MustRead.html").is_file())
            self.assertTrue((output / "licenses/data/fonts/NOTICE").is_file())
            self.assertFalse(list(output.rglob("*.pdf")))
            self.assertFalse(list(output.rglob("*.py")))
            self.assertFalse((output / "corpus").exists())
            with self.assertRaises(FileExistsError):
                package(output, wasm)

    def test_refuses_invalid_module_before_writing(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            wasm = root / "bad.wasm"
            wasm.write_bytes(b"not wasm")
            with self.assertRaises(ValueError):
                package(root / "site", wasm)
            self.assertFalse((root / "site").exists())
