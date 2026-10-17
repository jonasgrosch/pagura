"""Pinned corpus manifests and independent oracle selection."""
import json
from pathlib import Path
from .config import ROOT, MANIFESTS, read_json, sha
from .images import normalize, validate_tolerance


def manifest_digest():
    """One digest over every area manifest, so any fixture change is detected."""
    return sha(b"".join(p.name.encode() + b"\0" + p.read_bytes() for p in sorted(MANIFESTS.glob("*.json"))))


def render_cases():
    """Every independently oracled render case across all fixture areas."""
    return [case for path in sorted(MANIFESTS.glob("*.json")) for case in read_json(path)["cases"]
            if case.get("kind") == "pdf" and case["expected"] == "render"]


def selected_pages(selection, count):
    """CLI pages are one-based inclusive; internal page indices are zero-based."""
    if type(count) is not int or not 1 <= count <= 100_000:
        raise ValueError("manifest pages must be 1..100000")
    if selection == "all":
        return range(count)
    chosen = set()
    for entry in selection.split(","):
        bounds = entry.split("-")
        if len(bounds) not in (1, 2) or any(not part.isdecimal() for part in bounds):
            raise ValueError("pages must be all or one-based ranges such as 1-3,5")
        first, last = int(bounds[0]), int(bounds[-1])
        if not 1 <= first <= last <= count:
            raise ValueError("page range outside declared document")
        chosen.update(range(first - 1, last))
    return sorted(chosen)


def corpus_cases(directory, manifest, selection="all", dpis=(72,)):
    """Hash-pin arbitrary PDFs and optional independent RGB oracles before measuring.

    The manifest declares page count; it is not a conformance oracle. Neither
    content nor filenames are passed to a shell. Paths must stay in the corpus.
    """
    if not dpis or len(set(dpis)) != len(dpis) or any(type(dpi) is not int or not 1 <= dpi <= 9600 for dpi in dpis):
        raise ValueError("DPIs must be distinct integers in 1..9600")
    if directory is None:
        if manifest is not None:
            raise ValueError("--manifest requires --corpus")
        cases = []
        for case in render_cases():
            count = case.get("pages", 1)
            indices = selected_pages(selection, count)
            validate_tolerance(case.get("tolerance", 0))
            if "password" in case and not isinstance(case["password"], str):
                raise ValueError("password must be a string")
            if sha((ROOT / case["path"]).read_bytes()) != case["sha256"]:
                raise ValueError("changed generated PDF")
            for page in indices:
                for dpi in dpis:
                    clone = {**case, "path": str(ROOT / case["path"]), "page": page, "dpi": dpi, "page_count": count}
                    if dpi != case["dpi"] or page != 0:
                        clone.pop("oracle", None)
                        clone.pop("oracle_sha256", None)
                    elif sha((ROOT / case["oracle"]).read_bytes()) != case["oracle_sha256"]:
                        raise ValueError("changed generated oracle")
                    cases.append(clone)
        return cases, manifest_digest()
    directory = Path(directory).resolve()
    manifest = Path(manifest).resolve() if manifest else directory / "manifest.json"
    raw = manifest.read_bytes()
    data = json.loads(raw)
    if not isinstance(data, dict) or type(data.get("schema")) is not int or data["schema"] != 1 or not isinstance(data.get("documents"), list) or not data["documents"]:
        raise ValueError("expected corpus schema 1 with nonempty documents")
    cases, identifiers = [], set()

    def pinned(relative, digest):
        if not isinstance(relative, str) or Path(relative).is_absolute():
            raise ValueError("corpus paths must be relative")
        path = (directory / relative).resolve()
        if not path.is_relative_to(directory) or sha(path.read_bytes()) != digest:
            raise ValueError("escaping path or changed corpus file")
        return str(path)

    for document in data["documents"]:
        if not isinstance(document, dict) or not {"id", "path", "sha256", "pages"} <= document.keys():
            raise ValueError("document requires id, path, sha256 and pages")
        if not isinstance(document.get("oracles", []), list):
            raise ValueError("oracles must be a list")
        if "password" in document and not isinstance(document["password"], str):
            raise ValueError("password must be a string")
        validate_tolerance(document.get("tolerance", 0))
        identifier = document.get("id")
        if not isinstance(identifier, str) or not identifier or identifier in identifiers:
            raise ValueError("document IDs must be unique nonempty strings")
        identifiers.add(identifier)
        source = pinned(document["path"], document["sha256"])
        pages = selected_pages(selection, document["pages"])
        if "selected_pages" in document:
            selected = document["selected_pages"]
            if (not isinstance(selected, list) or not selected or
                    any(type(page) is not int or not 0 <= page < document["pages"] for page in selected) or
                    len(set(selected)) != len(selected)):
                raise ValueError("selected_pages must contain unique in-range zero-based pages")
            if selection != "all" and not set(pages) <= set(selected):
                raise ValueError("CLI page selection exceeds manifest selected_pages")
            pages = sorted(selected) if selection == "all" else pages
        oracles = {}
        for oracle in document.get("oracles", []):
            if not isinstance(oracle, dict) or not {"page", "dpi", "path", "sha256"} <= oracle.keys():
                raise ValueError("oracle requires page, dpi, path and sha256")
            key = (oracle["page"], oracle["dpi"])
            if (type(key[0]) is not int or not 0 <= key[0] < document["pages"]
                    or type(key[1]) is not int or not 1 <= key[1] <= 9600 or key in oracles):
                raise ValueError("invalid or duplicate oracle cell")
            path = pinned(oracle["path"], oracle["sha256"])
            normalize(Path(path).read_bytes())
            oracles[key] = (path, oracle["sha256"], validate_tolerance(oracle.get("tolerance", document.get("tolerance", 0))))
        for page in pages:
            for dpi in dpis:
                case = {"id": identifier, "path": source, "sha256": document["sha256"], "page": page, "dpi": dpi, "page_count": document["pages"]}
                case["tolerance"] = document.get("tolerance", 0)
                if "password" in document:
                    case["password"] = document["password"]
                if (page, dpi) in oracles:
                    case["oracle"], case["oracle_sha256"], case["tolerance"] = oracles[page, dpi]
                cases.append(case)
    return cases, sha(raw)

