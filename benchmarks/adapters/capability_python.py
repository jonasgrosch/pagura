"""Read-only PDF service probes through the pinned external Python bindings.

Only synthetic G01 inputs are passed by tools/capability_probes.py. Import or
adapter failure never becomes a claim that the underlying engine lacks a feature.
"""
import hashlib
import json
import sys


def pdfium(document, operation):
    if operation == "pages":
        return {"count": len(document), "sizes": [list(document.get_page_size(i)) for i in range(len(document))]}
    if operation == "metadata":
        return {key: document.get_metadata_value(key) for key in ("Title", "Author")}
    if operation == "attachments":
        result = []
        for index in range(document.count_attachments()):
            attachment = document.get_attachment(index)
            result.append({"name": attachment.get_name(), "sha256": hashlib.sha256(bytes(attachment.get_data())).hexdigest()})
        return sorted(result, key=lambda row: row["name"])
    if operation == "text":
        page = document[0]
        try:
            text = page.get_textpage()
            try:
                return text.get_text_range().replace("\r\n", "\n").strip("\n")
            finally:
                text.close()
        finally:
            page.close()
    raise ValueError("unknown operation")


def mupdf(document, operation):
    if operation == "pages":
        return {"count": document.page_count, "sizes": [[page.rect.width, page.rect.height] for page in document]}
    if operation == "metadata":
        return {key: document.metadata.get(key.lower(), "") or "" for key in ("Title", "Author")}
    if operation == "attachments":
        return sorted([{"name": name, "sha256": hashlib.sha256(document.embfile_get(name)).hexdigest()}
                       for name in document.embfile_names()], key=lambda row: row["name"])
    if operation == "text":
        return document[0].get_text().replace("\r\n", "\n").strip("\n")
    raise ValueError("unknown operation")


def probe(engine, operation, source):
    try:
        if engine == "pdfium":
            import pypdfium2 as backend  # pyright: ignore[reportMissingImports]
            opener, reader = backend.PdfDocument, pdfium
        elif engine == "mupdf":
            import pymupdf as backend  # pyright: ignore[reportMissingImports]
            opener, reader = backend.open, mupdf
        else:
            return {"status": "adapter-error", "reason": "unknown engine"}
    except ImportError:
        return {"status": "engine-unavailable", "reason": "pinned binding could not be imported"}
    try:
        document = opener(source)
    except Exception as error:
        # Engine open errors are observable rejections, never Unsupported.
        return {"status": "rejected", "reason": type(error).__name__}
    try:
        return {"status": "ok", "value": reader(document, operation)}
    except AttributeError:
        return {"status": "adapter-unavailable", "reason": "binding does not expose the expected API"}
    except Exception as error:
        return {"status": "adapter-error", "reason": type(error).__name__}
    finally:
        document.close()


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[2] == "--version":
        if sys.argv[1] == "pdfium":
            import pypdfium2 as binding  # pyright: ignore[reportMissingImports]
            version = {"wrapper": str(binding.PYPDFIUM_INFO), "engine": str(binding.PDFIUM_INFO)}
        elif sys.argv[1] == "mupdf":
            import pymupdf as binding  # pyright: ignore[reportMissingImports]
            version = {"wrapper": binding.VersionBind, "engine": binding.VersionFitz}
        else:
            raise SystemExit("unknown engine")
        print(json.dumps(version))
        raise SystemExit
    if len(sys.argv) != 4:
        raise SystemExit("expected ENGINE OPERATION SYNTHETIC_PDF")
    print(json.dumps({"schema": 1, **probe(*sys.argv[1:])}, sort_keys=True))
