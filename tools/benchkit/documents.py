"""One process and one open per document/DPI/engine conversion."""
import random
import re
import shutil
import tempfile
from pathlib import Path

from .config import ROOT, sha
from .images import normalize, pixel_difference
from .measurement import run_process, preserve_failure
from .reports import new_report
from .statistics import comparison_statistics
from .timings import parse_document_stages

GOOD = ("ok", "unverified")
METRICS = ("elapsed_ns", "engine_ns", "open_ns", "render_ns", "emit_ns")


def document_jobs(cases, bases):
    """Group selected pages without silently expanding work in either engine."""
    grouped = {}
    for case in cases:
        grouped.setdefault((case["id"], case["dpi"]), []).append(case)
    jobs = []
    for pages in grouped.values():
        pages.sort(key=lambda case: case["page"])
        first = pages[0]
        if any((case["path"], case["sha256"], case.get("page_count")) !=
               (first["path"], first["sha256"], first.get("page_count")) for case in pages):
            raise ValueError("conflicting document metadata")
        indices = [case["page"] for case in pages]
        if indices != list(range(indices[0], indices[-1] + 1)):
            raise ValueError("document mode requires one contiguous page range")
        count = first.get("page_count")
        if type(count) is not int or not 1 <= count <= 100_000 or indices[0] < 0 or indices[-1] >= count:
            raise ValueError("document mode requires a valid declared page_count")
        if any(name in bases for name in ("pagura", "revision")) and indices != list(range(count)):
            raise ValueError("Pagura convert has no range option; select the whole document or use --process-per-page")
        jobs.append(pages)
    return jobs


def document_command(name, base, pages, output):
    """Adapter ranges are zero-based; Poppler's range is one-based inclusive."""
    case = pages[0]
    source, dpi = str(case["path"]), str(case["dpi"])
    first, last = pages[0]["page"], pages[-1]["page"]
    if name in ("pagura", "revision"):
        if first != 0 or last + 1 != case["page_count"]:
            raise ValueError("Pagura document conversion requires all pages")
        return base + ["convert", "--timings", source, str(output), dpi]
    if name == "poppler":
        return base + ["-r", dpi, "-f", str(first + 1), "-l", str(last + 1), source, str(output / "page")]
    return base + [source, str(output), dpi, str(first), str(last)]


def expected_pages(pages):
    """Revalidate independent oracles outside timed regions."""
    expected = {}
    for case in pages:
        image = None
        if "oracle" in case:
            path = ROOT / case["oracle"]
            data = path.read_bytes()
            if sha(data) != case["oracle_sha256"]:
                raise ValueError("oracle changed after manifest validation")
            pixels = normalize(data)
            image = {"path": path, "sha256": case["oracle_sha256"], "pixels_sha256": sha(pixels[2])}
        expected[case["page"]] = image
    return expected


def measure_document(command, output, expected, policy, *, stages_required, image_sink):
    """Time one process, then verify every output and one global stage record.

    The caller supplies a new empty output directory per process. Shared open,
    elapsed and RSS fields appear only in the document sample, never each page.
    """
    if any(output.iterdir()):
        raise ValueError("document output directory must be empty")
    result, diagnostic = run_process(command, policy, log_budget=min(policy["maximum_output_bytes"], 1024 + 256 * len(expected)))
    indices = list(expected)
    result["pages"] = []
    if result["exit_code"] != 0 or result["status"] == "timeout":
        result["pages"] = [{"page": page, "status": result["status"], "exit_code": result["exit_code"]} for page in indices]
        return result
    files = {}
    output_error = None
    for path in output.iterdir():
        match = re.fullmatch(r"page-(\d+)\.ppm", path.name)
        if not match or not path.is_file():
            output_error = "unexpected document output"
            continue
        page = int(match[1]) - 1
        if page in files or page not in expected:
            output_error = "extra or duplicate page output"
        files[page] = path
    for page, pinned in expected.items():
        oracle = None
        if pinned is not None:
            data = pinned["path"].read_bytes()
            if sha(data) != pinned["sha256"]:
                raise ValueError("oracle changed after manifest validation")
            oracle = normalize(data)
        run = {"page": page, "exit_code": 0}
        path = files.get(page)
        try:
            if path is None:
                raise ValueError("missing page output")
            if path.stat().st_size > policy["maximum_output_bytes"]:
                raise ValueError("oversize output")
            data = path.read_bytes()
            actual = normalize(data)
            run.update(status="unverified" if oracle is None else "ok" if actual == oracle else "incorrect",
                       width=actual[0], height=actual[1], pixels_sha256=sha(actual[2]), output_sha256=sha(data))
            image_sink[page] = path
            if oracle is not None and actual != oracle:
                run["oracle_difference"] = pixel_difference(actual, oracle)
        except (ValueError, OSError) as error:
            run.update(status="invalid-output", diagnostic=str(error))
        if path is not None:
            preserve_failure(path, run, policy, str(output / str(page)))
        result["pages"].append(run)
    statuses = [run["status"] for run in result["pages"]]
    result["status"] = ("invalid-output" if output_error or "invalid-output" in statuses else
                        "incorrect" if "incorrect" in statuses else "unverified" if "unverified" in statuses else "ok")
    if output_error:
        result["diagnostic"] = output_error
    try:
        stages = parse_document_stages(diagnostic, indices, result["elapsed_ns"])
        if stages is None and stages_required:
            raise ValueError("missing stage timings")
        result["stages_status"] = "available" if stages else "unavailable"
        if stages is None:
            result["stages"] = None
        else:
            per_page = stages.pop("pages")
            result["stages"] = stages
            for run, timing in zip(result["pages"], per_page):
                run["stages"] = {key: timing[key] for key in ("render_ns", "emit_ns")}
    except ValueError as error:
        result.update(status="invalid-timings", diagnostic=str(error))
        for run in result["pages"]:
            run["status"] = "invalid-timings"
    return result


def _statistics(cell, policy, metrics):
    cell["statistics"] = {metric: comparison_statistics(cell, policy, metric) for metric in metrics}
    cell["acceptance"] = {"pass": False, "reasons": ["document conversion is exploratory; no page-level process timing"]}


def document_report(cases, bases, identities, policy, *, samples, warmups, seed, input_digest,
                    operation="conversion", revision=None, progress=None):
    """Pair complete document conversions, with per-page verification and stages."""
    jobs = document_jobs(cases, bases)
    report = new_report(identities, samples, warmups, seed, input_digest, operation, revision, process_per_page=False)
    rng = random.Random(seed)
    rng.shuffle(jobs)
    directory = ROOT / "benchmarks/results/local"
    directory.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="document-", dir=directory) as temporary:
        for job_index, pages in enumerate(jobs):
            case = pages[0]
            if sha(Path(case["path"]).read_bytes()) != case["sha256"]:
                raise ValueError("PDF changed after manifest validation")
            expected = expected_pages(pages)
            representatives = {}
            retained = Path(temporary) / f"representatives-{job_index}"
            retained.mkdir()
            baselines = [name for name in bases if name != "pagura"]
            rng.shuffle(baselines)
            for engine in baselines:
                common = {"case": case["id"], "dpi": case["dpi"], "engine": engine, "operation": operation,
                          "pdf_sha256": case["sha256"], "status": "unavailable", "commands": {}}
                verification = ("independent-oracle" if all(value is not None for value in expected.values()) else
                                "unverified" if all(value is None for value in expected.values()) else "mixed; includes unverified")
                document = {**common, "pages": list(expected), "verification": verification, "samples": [], "warmup_failures": []}
                cells = {}
                for page, oracle in expected.items():
                    cells[page] = {**common, "page": page, "samples": [], "warmup_failures": [],
                        "verification": "independent-oracle" if oracle else "unverified",
                        "oracle_pixels_sha256": oracle["pixels_sha256"] if oracle else None}
                report["document_cells"].append(document)
                report["cells"].extend(cells.values())
                if all(identities[name]["status"] == "available" for name in ("pagura", engine)):
                    document["status"] = "measured"
                    for cell in cells.values():
                        cell["status"] = "measured"
                    for pair in range(-warmups, samples):
                        order = ["pagura", engine]
                        rng.shuffle(order)
                        sample = {"pair": pair, "order": order}
                        images = {}
                        for name in order:
                            output = Path(temporary) / f"{job_index}-{engine}-{pair}-{name}"
                            output.mkdir()
                            command = document_command(name, bases[name], pages, output)
                            document["commands"][name] = command
                            decoded = {}
                            run = measure_document(command, output, expected, policy, stages_required=name != "poppler", image_sink=decoded)
                            sample["pagura" if name == "pagura" else "baseline"] = run
                            images[name] = decoded
                        if pair >= 0:
                            document["samples"].append(sample)
                        elif any(sample[key]["status"] not in GOOD for key in ("pagura", "baseline")):
                            document["warmup_failures"].append(sample)
                        page_runs = {key: {run["page"]: run for run in sample[key]["pages"]} for key in ("pagura", "baseline")}
                        for page in cells:
                            page_sample = {"pair": pair, "order": order}
                            for name, key in (("pagura", "pagura"), (engine, "baseline")):
                                page_sample[key] = page_runs[key][page]
                                if pair >= 0 and page_sample[key]["status"] in (*GOOD, "incorrect"):
                                    if (name, page) not in representatives:
                                        target = retained / (sha(f"{name}/{page}".encode()) + ".ppm")
                                        shutil.copyfile(images[name][page], target)
                                        representatives[name, page] = (pair, target, page_sample[key]["pixels_sha256"])
                            if pair >= 0:
                                if all(page in images[name] for name in order):
                                    page_sample["agreement"] = pixel_difference(normalize(images["pagura"][page].read_bytes()), normalize(images[engine][page].read_bytes()))
                                cells[page]["samples"].append(page_sample)
                            elif any(page_sample[key]["status"] not in GOOD for key in ("pagura", "baseline")):
                                cells[page]["warmup_failures"].append(page_sample)
                        for name in order:
                            shutil.rmtree(Path(temporary) / f"{job_index}-{engine}-{pair}-{name}")
                _statistics(document, policy, METRICS)
                for cell in cells.values():
                    _statistics(cell, policy, ("render_ns", "emit_ns"))
                if progress:
                    progress(report)
            for page in expected:
                names = sorted(name for name, index in representatives if index == page)
                for index, first in enumerate(names):
                    for second in names[index + 1:]:
                        a_pair, a_path, a_hash = representatives[first, page]
                        b_pair, b_path, b_hash = representatives[second, page]
                        a, b = normalize(a_path.read_bytes()), normalize(b_path.read_bytes())
                        report["agreement"].append({"case": case["id"], "dpi": case["dpi"], "page": page,
                            "engines": [first, second], "pairs": [a_pair, b_pair], "pixels_sha256": [a_hash, b_hash],
                            "scope": "first decoded measured output per engine; investigation only", **pixel_difference(a, b)})
            shutil.rmtree(retained)
    report["gate"] = {"pass": False, "reasons": ["conversion/regression evidence is exploratory and ineligible for release"]}
    return report
