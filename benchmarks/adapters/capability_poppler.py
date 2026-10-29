"""G01 through the documented Poppler GLib API, using the OS GI binding.

The hosted runner supplies this API separately from the pinned Python wheels.
Only synthetic fixtures are accepted by the invoking harness. Attachment names
never become filesystem paths, and engine diagnostics never enter the report.
"""
import hashlib
import json
import platform
import sys
import tempfile
from pathlib import Path


def read(document, operation):
    if operation == "pages":
        count = document.get_n_pages()
        return {"count": count, "sizes": [list(document.get_page(i).get_size()) for i in range(count)]}
    if operation == "metadata":
        return {"Title": document.get_title() or "", "Author": document.get_author() or ""}
    if operation == "text":
        return (document.get_page(0).get_text() or "").replace("\r\n", "\n").strip("\n")
    if operation == "attachments":
        result = []
        with tempfile.TemporaryDirectory(prefix="poppler-service-") as temporary:
            destination = Path(temporary) / "attachment.bin"
            for attachment in document.get_attachments() or []:
                if not attachment.save(str(destination)):
                    raise ValueError("attachment save failed")
                result.append({"name": attachment.get_name(), "sha256": hashlib.sha256(destination.read_bytes()).hexdigest()})
                destination.unlink()
        return sorted(result, key=lambda row: row["name"])
    raise ValueError("unknown operation")


def main():
    import gi  # pyright: ignore[reportMissingImports]
    gi.require_version("Poppler", "0.18")
    from gi.repository import Poppler  # pyright: ignore[reportMissingImports]
    if sys.argv[1:] in (["--version"], ["-v"]):
        print(json.dumps({"engine": Poppler.get_version(), "wrapper": "PyGObject " + gi.__version__, "python": platform.python_version()}))
        return
    operation, source = sys.argv[1:]
    try:
        document = Poppler.Document.new_from_file(Path(source).resolve().as_uri(), None)
    except Exception as error:
        response = {"status": "rejected", "reason": type(error).__name__}
    else:
        try:
            response = {"status": "ok", "value": read(document, operation)}
        except AttributeError:
            response = {"status": "adapter-unavailable", "reason": "installed GLib binding lacks the expected public API"}
        except Exception as error:
            response = {"status": "adapter-error", "reason": type(error).__name__}
    print(json.dumps({"schema": 1, **response}, sort_keys=True))


if __name__ == "__main__":
    main()
