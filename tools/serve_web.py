"""Serve the dependency-free viewer and built WASI command on loopback only.

Usage: cargo wasm && python3 tools/serve_web.py [--port 8000] [--fixtures]
No PDFs are uploaded. --fixtures exposes only generated development fixtures.
"""
import argparse
import functools
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]
WEB = ROOT / "web"
WASM = ROOT / "target/wasm32-wasip1/release/pagura-host.wasm"
FIXTURES = ROOT / "corpus/generated"


class ViewerHandler(SimpleHTTPRequestHandler):
    """Isolated static files; generated fixtures require an explicit opt-in."""

    extensions_map = {**SimpleHTTPRequestHandler.extensions_map,
                      ".mjs": "text/javascript", ".wasm": "application/wasm"}

    def __init__(self, *args, fixtures=False, **kwargs):
        self.fixtures_enabled = fixtures
        super().__init__(*args, **kwargs)

    def end_headers(self):
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cross-Origin-Resource-Policy", "same-origin")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Security-Policy",
                         "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; "
                         "worker-src 'self'; style-src 'self' 'unsafe-inline'; "
                         "connect-src 'self'; img-src 'self' data:; object-src 'none'; "
                         "base-uri 'none'; frame-ancestors 'none'; form-action 'none'")
        super().end_headers()

    def resolve_path(self, path):
        """Map a URL to one allowed root, checking symlinks after resolution."""
        path = unquote(urlsplit(path).path, errors="strict")
        if path == "/pagura-host.wasm":
            return WASM
        if path == "/fixtures" or path.startswith("/fixtures/"):
            if not self.fixtures_enabled:
                return None
            root, relative = FIXTURES, path.removeprefix("/fixtures").lstrip("/")
        else:
            root, relative = WEB, path.lstrip("/")
        try:
            translated = (root / relative).resolve()
            if translated.is_dir():
                translated = (translated / "index.html").resolve()
            if translated.is_relative_to(root.resolve()):
                return translated
        except (OSError, RuntimeError, ValueError):
            pass
        return None

    def translate_path(self, path):
        # send_head rejects denied routes before the base handler opens a file.
        translated = self.resolve_path(path)
        return str(translated) if translated is not None else str(WEB / "__missing__")

    def list_directory(self, path):
        self.send_error(403, "Directory listing is disabled")
        return None

    def send_head(self):
        """Apply the same containment rules to GET and HEAD."""
        try:
            path = unquote(urlsplit(self.path).path, errors="strict")
        except (UnicodeError, ValueError):
            self.send_error(400, "Invalid URL path")
            return None
        if ".." in path.split("/") or "\\" in path or "\0" in path:
            self.send_error(403, "Parent paths are not served")
            return None
        if self.resolve_path(self.path) is None:
            self.send_error(404, "File not found")
            return None
        return super().send_head()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=8000)
    parser.add_argument("--fixtures", action="store_true",
                        help="development only: serve corpus/generated read-only at /fixtures/")
    args = parser.parse_args()
    if not WASM.is_file():
        parser.error("missing pagura-host.wasm; run cargo wasm first")
    handler = functools.partial(ViewerHandler, directory=str(WEB), fixtures=args.fixtures)
    with ThreadingHTTPServer(("127.0.0.1", args.port), handler) as server:
        print(f"Pagura: http://localhost:{server.server_port} (COOP/COEP enabled)", flush=True)
        try:
            server.serve_forever()
        except KeyboardInterrupt:
            pass


if __name__ == "__main__":
    main()
