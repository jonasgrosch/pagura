"""Run real Chrome Canvas regressions using only the Python standard library.

Designed for hosted Ubuntu runners with Chrome installed. Never uses an existing
browser profile. JSON and PNG evidence is retained in target/thumbnail-pixels.
"""
import base64
import functools
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import tempfile
import threading
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[1]


def main():
    chrome = os.environ.get("CHROME_BIN") or shutil.which("google-chrome") or shutil.which("chromium")
    if not chrome:
        raise SystemExit("Chrome is required; run the thumbnail-pixels hosted job")
    subprocess.run([chrome, "--version"], check=True)
    output = ROOT / "target/thumbnail-pixels"
    output.mkdir(parents=True, exist_ok=True)
    completed, reports = threading.Event(), []

    class Handler(SimpleHTTPRequestHandler):
        def log_message(self, format: str, *_args: object) -> None:
            pass

        def do_POST(self):
            length = int(self.headers.get("Content-Length", "0"))
            if self.path != "/report" or not 0 < length <= 8 * 1024 * 1024:
                self.send_error(400)
                return
            reports.append(json.loads(self.rfile.read(length)))
            self.send_response(204)
            self.end_headers()
            completed.set()

    server = ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Handler, directory=str(ROOT)))
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="pagura-pixels-") as profile, (output / "chrome.log").open("w") as log:
            process = subprocess.Popen([
                chrome, "--headless", "--no-sandbox", "--no-first-run", "--no-default-browser-check",
                "--disable-background-networking", f"--user-data-dir={profile}",
                f"http://127.0.0.1:{server.server_port}/tools/thumbnail_pixels.html",
            ], stdout=log, stderr=log, start_new_session=True)
            try:
                if not completed.wait(45):
                    raise RuntimeError(f"Browser pixel tests timed out; see {output / 'chrome.log'}")
            finally:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
    report = reports[0]
    for name, data in report.pop("images", {}).items():
        assert re.fullmatch(r"[a-z0-9-]+", name) and data.startswith("data:image/png;base64,"), "invalid image artifact"
        (output / f"{name}.png").write_bytes(base64.b64decode(data.split(",", 1)[1], validate=True))
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2), flush=True)
    assert not report.get("error"), report.get("error")
    assert len(report.get("results", [])) == 19, "missing pixel test results"
    assert all(result.get("passed") for result in report["results"]), "thumbnail pixel regression"


if __name__ == "__main__":
    main()
