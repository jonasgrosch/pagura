"""External PDFium/MuPDF conversions; stage contract is in benchmarks/README.md."""
import json
import sys
from pathlib import Path
from time import perf_counter_ns

engine = sys.argv[1]
if engine == "pdfium":
    import pypdfium2 as backend
    if sys.argv[2] == "--version":
        print(json.dumps({"wrapper": str(backend.PYPDFIUM_INFO), "engine": str(backend.PDFIUM_INFO)}))
        raise SystemExit
elif engine == "mupdf":
    import pymupdf as backend
    if sys.argv[2] == "--version":
        print(json.dumps({"wrapper": backend.VersionBind, "engine": backend.VersionFitz}))
        raise SystemExit
else:
    raise ValueError("unknown engine")

args = sys.argv[2:]
if len(args) not in (4, 5):
    raise ValueError("expected INPUT OUTPUT DPI PAGE or INPUT OUTDIR DPI FIRST LAST (zero-based)")
source, target, dpi = args[:3]
first, last = int(args[3]), int(args[-1])
if not 0 <= first <= last:
    raise ValueError("invalid page range")
started = perf_counter_ns()
document = backend.PdfDocument(source) if engine == "pdfium" else backend.open(source)
try:
    count = len(document) if engine == "pdfium" else document.page_count
    indices = range(count)  # page-list metadata, without loading individual pages
    if last not in indices:
        raise ValueError("page range outside document")
    opened = perf_counter_ns()
    stages = []
    for index in range(first, last + 1):
        render_started = perf_counter_ns()
        selected = document[index]  # page loading belongs to render
        try:
            if engine == "pdfium":
                raster = selected.render(scale=int(dpi) / 72, draw_annots=True)
            else:
                raster = selected.get_pixmap(matrix=backend.Matrix(int(dpi) / 72, int(dpi) / 72),
                                             colorspace=backend.csRGB, alpha=False, annots=True)
            try:
                rendered = perf_counter_ns()
                output = Path(target) / f"page-{index + 1:04}.ppm" if len(args) == 5 else Path(target)
                if engine == "pdfium":
                    image = raster.to_pil().convert("RGB")
                    image.save(output, format="PPM")
                else:
                    output.write_bytes(raster.tobytes("ppm"))
                emitted = perf_counter_ns()
                stages.append({"page": index, "render_ns": rendered - render_started,
                               "emit_ns": emitted - rendered})
            finally:
                if engine == "pdfium":
                    raster.close()
        finally:
            if engine == "pdfium":
                selected.close()
    total = perf_counter_ns() - started
finally:
    document.close()
print(json.dumps({"schema": 1, "open_ns": opened - started, "pages": stages, "total_ns": total}))
