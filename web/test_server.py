"""Offline HTTP handler checks using in-memory connections, never a socket bind."""
import importlib.util
import io
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("pagura_serve_web", ROOT / "tools/serve_web.py")
SERVER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SERVER)


class Connection:
    """Only the two connection methods used by the standard-library handler."""

    def __init__(self, method, path):
        self.input = io.BytesIO(f"{method} {path} HTTP/1.0\r\n\r\n".encode())
        self.output = bytearray()

    def makefile(self, *_args):
        return self.input

    def sendall(self, data):
        self.output.extend(data)


class QuietHandler(SERVER.ViewerHandler):
    """Keep successful checks concise."""

    def log_message(self, *_args):
        pass


class ServerTests(unittest.TestCase):
    def response(self, path, method="GET", *, fixtures=False):
        connection = Connection(method, path)
        QuietHandler(connection, ("127.0.0.1", 0), SimpleNamespace(),
                     directory=str(SERVER.WEB), fixtures=fixtures)
        return bytes(connection.output).split(b"\r\n\r\n", 1)

    def test_files_mime_isolation_and_private_paths(self):
        for path, code in [("/", b"200"), ("/viewer.mjs", b"200"),
                           ("/pagura-host.wasm", b"200"),
                           ("/../Cargo.toml", b"403"),
                           ("/%2e%2e/Cargo.toml", b"403"),
                           ("/Cargo.toml", b"404")]:
            with self.subTest(path=path):
                headers, _body = self.response(path)
                self.assertEqual(headers.split(b" ")[1], code)
                for header in [b"Cross-Origin-Opener-Policy: same-origin",
                               b"Cross-Origin-Embedder-Policy: require-corp",
                               b"Cross-Origin-Resource-Policy: same-origin",
                               b"X-Content-Type-Options: nosniff"]:
                    self.assertIn(header, headers)
        headers, body = self.response("/pagura-host.wasm")
        self.assertIn(b"Content-type: application/wasm", headers)
        self.assertEqual(body, SERVER.WASM.read_bytes())
        headers, body = self.response("/viewer.mjs")
        self.assertIn(b"Content-type: text/javascript", headers)
        self.assertEqual(body, (SERVER.WEB / "viewer.mjs").read_bytes())
        _headers, body = self.response("/pagura-host.wasm", "HEAD")
        self.assertEqual(body, b"")

    def test_fixture_routes_are_opt_in_read_only_and_isolated(self):
        path = "/fixtures/classic/two-pages.pdf"
        expected = (SERVER.FIXTURES / "classic/two-pages.pdf").read_bytes()
        for method in ["GET", "HEAD"]:
            headers, _body = self.response(path, method)
            self.assertEqual(headers.split(b" ")[1], b"404")
            headers, body = self.response(path, method, fixtures=True)
            self.assertEqual(headers.split(b" ")[1], b"200")
            self.assertIn(b"Content-type: application/pdf", headers)
            self.assertIn(b"Cross-Origin-Opener-Policy: same-origin", headers)
            self.assertIn(b"Cross-Origin-Embedder-Policy: require-corp", headers)
            self.assertEqual(body, expected if method == "GET" else b"")
        headers, body = self.response("/fixtures/classic/two-pages.ppm", fixtures=True)
        self.assertEqual(headers.split(b" ")[1], b"200")
        self.assertEqual(body, (SERVER.FIXTURES / "classic/two-pages.ppm").read_bytes())
        for method in ["POST", "PUT", "DELETE"]:
            headers, _body = self.response(path, method, fixtures=True)
            self.assertEqual(headers.split(b" ")[1], b"501")
        self.assertEqual((SERVER.FIXTURES / "classic/two-pages.pdf").read_bytes(), expected)
        for method in ["GET", "HEAD"]:
            for path in ["/fixtures/../Cargo.toml", "/fixtures/%2e%2e/Cargo.toml",
                         "/fixtures/classic/%2e%2e%2f%2e%2e/Cargo.toml",
                         "/fixtures/classic/%5c..%5cCargo.toml"]:
                with self.subTest(method=method, path=path):
                    headers, _body = self.response(path, method, fixtures=True)
                    self.assertEqual(headers.split(b" ")[1], b"403")

    def test_fixture_symlinks_cannot_escape_the_generated_root(self):
        original = SERVER.FIXTURES
        try:
            with tempfile.TemporaryDirectory(prefix="pagura-fixtures-") as temporary:
                SERVER.FIXTURES = Path(temporary)
                (SERVER.FIXTURES / "escape").symlink_to(ROOT, target_is_directory=True)
                (SERVER.FIXTURES / "secret.pdf").symlink_to(ROOT / "Cargo.toml")
                for method in ["GET", "HEAD"]:
                    for path in ["/fixtures/escape/Cargo.toml", "/fixtures/secret.pdf",
                                 "/fixtures/", "/fixtures/missing.pdf"]:
                        headers, body = self.response(path, method, fixtures=True)
                        self.assertEqual(headers.split(b" ")[1], b"404")
                        self.assertNotIn(b"[workspace]", body)
        finally:
            SERVER.FIXTURES = original

    def test_escaping_symlinks_and_directory_indexes_are_not_served(self):
        original = SERVER.WEB
        try:
            with tempfile.TemporaryDirectory(prefix="pagura-web-") as temporary:
                SERVER.WEB = Path(temporary)
                (SERVER.WEB / "escape").symlink_to(ROOT, target_is_directory=True)
                (SERVER.WEB / "nested").mkdir()
                (SERVER.WEB / "nested/index.html").symlink_to(ROOT / "Cargo.toml")
                for path in ["/escape/Cargo.toml", "/nested/"]:
                    headers, body = self.response(path)
                    self.assertEqual(headers.split(b" ")[1], b"404")
                    self.assertNotIn(b"[workspace]", body)
        finally:
            SERVER.WEB = original


if __name__ == "__main__":
    unittest.main()
