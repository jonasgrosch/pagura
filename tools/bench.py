"""CLI for verified process, document conversion and regression benchmarks."""
import argparse
import json
from pathlib import Path
from benchkit.config import read_json
from benchkit.engines import engines, identify
from benchkit.conversion import run_conversion
from benchkit.legacy import run
from benchkit.gate import gate
from benchkit.reports import write_summary


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("doctor")
    measure_args = sub.add_parser("run")
    measure_args.add_argument("--engines", default="pagura,poppler,pdfium,mupdf,pdfjs,hayro,quartz")
    measure_args.add_argument("--suite", choices=["smoke", "release"], default="smoke")
    measure_args.add_argument("--samples", type=int, default=10)
    measure_args.add_argument("--warmups", type=int, default=3)
    measure_args.add_argument("--seed", type=int, default=42017)
    measure_args.add_argument("--output")
    for name in ("convert", "regress"):
        convert_args = sub.add_parser(name)
        convert_args.add_argument("--corpus", type=Path)
        convert_args.add_argument("--manifest", type=Path)
        convert_args.add_argument("--pages", default="all")
        convert_args.add_argument("--dpis", default="72")
        convert_args.add_argument("--samples", type=int, default=10)
        convert_args.add_argument("--warmups", type=int, default=3)
        convert_args.add_argument("--seed", type=int, default=42017)
        convert_args.add_argument("--output")
        convert_args.add_argument("--no-rss", action="store_true", help="omit unavailable RSS instrumentation; blocks release acceptance")
        convert_args.add_argument("--process-per-page", action="store_true", default=name == "regress",
                                  help="relaunch and reopen for every page (default for regress)")
        if name == "convert":
            convert_args.add_argument("--engines", default="pagura,poppler,pdfium,mupdf,pdfjs,hayro,quartz")
        else:
            convert_args.add_argument("--revision", required=True)
    summary_args = sub.add_parser("summary")
    summary_args.add_argument("report")
    summary_args.add_argument("output")
    gate_args = sub.add_parser("gate")
    gate_args.add_argument("report")
    args = parser.parse_args(argv)
    if args.command == "doctor":
        print(json.dumps({k: identify(k, v) for k, v in engines().items()}, indent=2))
    elif args.command in ("convert", "regress"):
        run_conversion(args)
    elif args.command == "summary":
        write_summary(args.report, args.output)
    elif args.command == "run":
        if not 2 <= args.samples <= 1000 or not 0 <= args.warmups <= 100:
            parser.error("samples must be 2..1000 and warmups 0..100")
        run(args)
    else:
        verdict = gate(read_json(args.report))
        print(json.dumps(verdict, indent=2))
        raise SystemExit(0 if verdict["pass"] else 2)


if __name__ == "__main__":
    main()
