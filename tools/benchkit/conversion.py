"""Paired conversion orchestration; per-page mode remains explicit."""
import json
import random
import subprocess
import tempfile
from pathlib import Path
from .config import ROOT, POLICY_PATH, read_json, sha
from .corpus import corpus_cases
from .engines import engines, identify, conversion_command, revision_engine
from .images import normalize, pixel_difference
from .measurement import measure, preserve_failure
from .statistics import comparison_statistics, evaluate_cell
from .reports import local_report_path, conversion_markdown, new_report


def page_report(cases, bases, identities, policy, *, samples, warmups, seed, input_digest,
                      operation="conversion", revision=None, progress=None):
    """Measure randomized process pairs. Each page reopens its PDF in both engines."""
    if "pagura" not in bases or len(bases) < 2 or not 2 <= samples <= 1000 or not 0 <= warmups <= 100:
        raise ValueError("need pagura, a baseline, 2..1000 samples and 0..100 warmups")
    if not cases:
        raise ValueError("empty conversion corpus")
    report = new_report(identities, samples, warmups, seed, input_digest, operation, revision, process_per_page=True)
    report["measurement_options"] = {"rss_enabled": policy.get("measure_rss", True)}
    rng = random.Random(seed)
    jobs = list(cases)
    rng.shuffle(jobs)
    directory = ROOT / "benchmarks/results/local"
    directory.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="conversion-", dir=directory) as temporary:
        for case in jobs:
            # The verified hashes protect both files consumed here and those used by children.
            if sha(Path(case["path"]).read_bytes()) != case["sha256"]:
                raise ValueError("PDF changed after manifest validation")
            expected = None
            if "oracle" in case:
                oracle = Path(case["oracle"]) if Path(case["oracle"]).is_absolute() else ROOT / case["oracle"]
                data = oracle.read_bytes()
                if sha(data) != case["oracle_sha256"]:
                    raise ValueError("oracle changed after manifest validation")
                expected = normalize(data)
            representatives = {}
            baselines = [name for name in bases if name != "pagura"]
            rng.shuffle(baselines)
            for engine in baselines:
                cell = {"case": case["id"], "page": case["page"], "dpi": case["dpi"], "engine": engine,
                        "operation": operation, "pdf_sha256": case["sha256"],
                        "oracle_pixels_sha256": sha(expected[2]) if expected else None,
                        "verification": "independent-oracle" if expected else "unverified",
                        "status": "unavailable", "samples": [], "warmup_failures": [], "commands": {}}
                report["cells"].append(cell)
                if all(identities[name]["status"] == "available" for name in ("pagura", engine)):
                    cell["status"] = "measured"
                    for pair in range(-warmups, samples):
                        order = ["pagura", engine]
                        rng.shuffle(order)
                        sample = {"pair": pair, "order": order}
                        images = {}
                        for name in order:
                            output = Path(temporary) / (name + ".ppm")
                            command = conversion_command(name, bases[name], case, output)
                            cell["commands"][name] = command
                            decoded = []
                            result = measure(command, output, expected, policy, page=case["page"],
                                             stages_required=name != "poppler", image_sink=decoded, tolerance=case.get("tolerance", 0))
                            sample["pagura" if name == "pagura" else "baseline"] = result
                            preserve_failure(output, result, policy,
                                             f"{temporary}/{case['id']}/{case['page']}/{case['dpi']}/{name}/{pair}")
                            if pair >= 0 and result["status"] in ("ok", "unverified", "incorrect"):
                                image = decoded[0]
                                images[name] = image
                                representatives.setdefault(name, (pair, image))
                        if pair >= 0:
                            if len(images) == 2:
                                sample["agreement"] = pixel_difference(images["pagura"], images[engine])
                            cell["samples"].append(sample)
                        elif any(sample[key]["status"] not in ("ok", "unverified") for key in ("pagura", "baseline")):
                            cell["warmup_failures"].append(sample)
                cell["statistics"] = {metric: comparison_statistics(cell, policy, metric)
                                      for metric in ("elapsed_ns", "engine_ns", "open_ns", "render_ns", "emit_ns")}
                # A per-cell gate remains fail-closed for unverified results. Regression and
                # conversion ratios are exploratory even if an oracled cell beats the threshold.
                cell["acceptance"] = {"metric": "elapsed_ns", "scope": "process-only diagnostic; track ineligible for release",
                                      **evaluate_cell(cell, policy)}
                if progress:
                    progress(report)
            names = sorted(representatives)
            for index, first in enumerate(names):
                for second in names[index + 1:]:
                    a_pair, a = representatives[first]
                    b_pair, b = representatives[second]
                    report["agreement"].append({"case": case["id"], "page": case["page"], "dpi": case["dpi"],
                        "engines": [first, second], "pairs": [a_pair, b_pair],
                        "pixels_sha256": [sha(a[2]), sha(b[2])],
                        "scope": "first successful measured output per engine; investigation only",
                        **pixel_difference(a, b)})
    report["gate"] = {"pass": False, "reasons": ["conversion/regression evidence is exploratory and ineligible for release"]}
    return report


def run_conversion(args):
    if not 2 <= args.samples <= 1000 or not 0 <= args.warmups <= 100:
        raise ValueError("samples must be 2..1000 and warmups 0..100")
    policy = read_json(POLICY_PATH)
    policy["measure_rss"] = not args.no_rss
    cases, digest = corpus_cases(args.corpus, args.manifest, args.pages, tuple(int(dpi) for dpi in args.dpis.split(",")))
    if not args.process_per_page:
        from .documents import document_jobs
        document_jobs(cases, {"pagura": None})
    path = local_report_path(args.output, args.command + ".json")
    subprocess.run(["cargo", "build", "--release", "--locked", "--offline", "-p", "pagura"], cwd=ROOT, check=True)

    def execute(bases, revision=None):
        identities = {name: identify(name, base) for name, base in bases.items()}
        if revision is not None:
            identities["revision"]["commit"] = revision
        def save(report):
            # Checkpoints are deliberately gate-blocked even if execution stops early.
            path.write_text(json.dumps(report, indent=2) + "\n")
        report = conversion_report(cases, bases, identities, policy, samples=args.samples, warmups=args.warmups,
                                   seed=args.seed, input_digest=digest, operation="regression" if revision else "conversion",
                                   revision=revision, progress=save, process_per_page=args.process_per_page)
        save(report)
        path.with_suffix(".md").write_text(conversion_markdown(report))
        print(f"Report: {path}\nRelease gate: BLOCKED (exploratory track)")

    if args.command == "regress":
        with revision_engine(args.revision) as (command, commit):
            execute({"pagura": engines()["pagura"], "revision": command}, commit)
    else:
        requested = args.engines.split(",")
        available = engines()
        if "pagura" not in requested or len(set(requested)) != len(requested) or len(requested) < 2 or set(requested) - available.keys():
            raise ValueError("include pagura once, at least one known baseline, and no duplicates")
        execute({name: available[name] for name in requested})


def conversion_report(cases, bases, identities, policy, *, samples, warmups, seed, input_digest,
                      operation="conversion", revision=None, progress=None, process_per_page=False):
    """Default to one open per document; page-process measurements are an explicit track."""
    if "pagura" not in bases or len(bases) < 2 or not cases or not 2 <= samples <= 1000 or not 0 <= warmups <= 100:
        raise ValueError("need a nonempty corpus, pagura, baseline, 2..1000 samples and 0..100 warmups")
    from .documents import document_report
    run = page_report if process_per_page else document_report
    return run(cases, bases, identities, policy, samples=samples, warmups=warmups, seed=seed,
               input_digest=input_digest, operation=operation, revision=revision, progress=progress)
