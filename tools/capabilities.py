"""Validate the source capability inventory and its generated review view (G00).

No engines run here. Schema 2 records adjudicated source observations, API
families and unprobed comparison cells; executable evidence belongs to G01.
"""
import argparse
import hashlib
import json
import re
from collections import Counter
from pathlib import Path
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
LEDGER = Path("benchmarks/capabilities.json")
VIEW = Path("docs/CAPABILITIES.md")
LITERAL = re.compile(r'Feature::new\s*\(\s*"([^"\\]+)"\s*\)')
SURFACES = {"library", "cli", "host", "viewer"}
STATES = {"implemented", "partial", "missing"}
REVIEWS = {"gap", "boundary", "dispatch", "dormant", "mixed"}


def require(condition, message: str) -> None:
    if not condition:
        raise ValueError(message)


def read_json(path: Path):
    """Reject duplicate keys rather than silently losing a comparison or row."""
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, f"duplicate JSON key: {key}")
            result[key] = value
        return result
    return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=unique)


def local_file(root: Path, name: str) -> Path:
    require(isinstance(name, str) and bool(name), "empty evidence path")
    path = Path(name)
    require(not path.is_absolute() and ".." not in path.parts, f"escaping evidence path: {name}")
    target = (root / path).resolve()
    require(target.is_relative_to(root.resolve()) and target.is_file(), f"missing or escaping evidence: {name}")
    return target


def source_hash(path: Path) -> str:
    """Hash UTF-8 source with LF newlines, independent of checkout CRLF policy."""
    return hashlib.sha256(path.read_text(encoding="utf-8").encode("utf-8")).hexdigest()


def feature_tags(root: Path) -> dict[str, list[str]]:
    """Lexical candidates, not a Rust parser, reachability proof or gap list.

    Separate test files are excluded; inline tests/comments can still appear.
    Dynamic tags, macro expansion and untagged API gaps need manual G00 review.
    """
    found: dict[str, set[str]] = {}
    for path in sorted((root / "crates").glob("*/src/**/*.rs")):
        if any(part in {"tests", "tests.rs", "testing.rs"} for part in path.relative_to(root).parts):
            continue
        for tag in LITERAL.findall(path.read_text(encoding="utf-8")):
            found.setdefault(tag, set()).add(path.relative_to(root).as_posix())
    return {tag: sorted(paths) for tag, paths in sorted(found.items())}


def identifiers(root: Path, directory: str, collection: str) -> dict[str, str]:
    found: dict[str, str] = {}
    for path in sorted((root / directory).glob("*.json")):
        for row in read_json(path)[collection]:
            require(row["id"] not in found, f"duplicate {collection} ID: {row['id']}")
            found[row["id"]] = path.relative_to(root).as_posix()
    return found


def strings(value, label: str, *, nonempty: bool = True) -> list[str]:
    require(isinstance(value, list), f"{label}: expected a list")
    require(all(isinstance(item, str) and item.strip() for item in value), f"{label}: expected nonempty strings")
    require(len(set(value)) == len(value), f"{label}: duplicate values")
    require(bool(value) or not nonempty, f"{label}: empty list")
    return value


def validate(data, root: Path) -> None:
    require(isinstance(data, dict) and type(data.get("schema")) is int and data["schema"] == 2, "unsupported capability schema")
    require(set(data) == {"schema", "audited_revision", "evidence_kind", "engines", "source_files", "capabilities", "feature_tags", "api_inventory"}, "unexpected capability inventory fields")
    require(re.fullmatch(r"[0-9a-f]{40}", data.get("audited_revision", "")), "expected audited commit SHA")
    require(data.get("evidence_kind") == "source-inspection", "schema 2 is source inspection only")
    engines = read_json(root / "benchmarks/policy.json")["required_engines"]
    require(data.get("engines") == engines, "baseline engines differ from the release policy")
    packages = set(re.findall(r"^## ([A-Z]\d{2}) —", (root / "docs/COMPETITIVE_PARITY.md").read_text(encoding="utf-8"), re.M))
    requirements = identifiers(root, "references/requirements", "requirements")
    fixtures = identifiers(root, "corpus/manifests", "cases")
    sources = data.get("source_files")
    if not isinstance(sources, dict) or not sources:
        raise ValueError("source evidence hashes are missing")
    for name, digest in sources.items():
        require(isinstance(digest, str) and re.fullmatch(r"[0-9a-f]{64}", digest), f"invalid source hash: {name}")
        actual = source_hash(local_file(root, name))
        require(actual == digest, f"source changed; review affected capabilities before updating hash: {name}")
    rows = data.get("capabilities")
    if not isinstance(rows, list) or not rows:
        raise ValueError("empty capability inventory")
    seen = set()
    used_sources = set()
    assigned_requirements = set()
    for row in rows:
        require(isinstance(row, dict), "capability row must be an object")
        require(set(row) == {"id", "package", "operation", "feature", "owners", "surfaces", "implemented", "remaining", "sources", "tests", "requirements", "fixtures", "comparisons"}, "unexpected capability row fields")
        identity = row.get("id", "")
        require(isinstance(identity, str) and re.fullmatch(r"[a-z][a-z0-9.-]+", identity), "invalid capability ID")
        require(identity not in seen, f"duplicate capability: {identity}")
        seen.add(identity)
        require(row.get("package") in packages, f"{identity}: unknown work package")
        for field in ("operation", "feature", "implemented", "remaining"):
            require(isinstance(row.get(field), str) and bool(row[field].strip()), f"{identity}: missing {field}")
        for owner in strings(row.get("owners"), f"{identity}.owners"):
            require((root / "crates" / owner / "Cargo.toml").is_file() or owner in {"web", "tools"}, f"{identity}: unknown owner {owner}")
        layers = row.get("surfaces")
        require(isinstance(layers, dict) and set(layers) == SURFACES, f"{identity}: incomplete surface inventory")
        require(all(value in STATES for value in layers.values()), f"{identity}: invalid surface state")
        # No supported/N/A status can be inferred from marketing or source tags.
        comparisons = row.get("comparisons")
        require(isinstance(comparisons, dict) and set(comparisons) == set(engines), f"{identity}: missing baseline comparison")
        require(all(value == "unprobed" for value in comparisons.values()), f"{identity}: source inventory cannot assert runtime support or N/A; link G01 evidence separately")
        required = strings(row.get("requirements"), f"{identity}.requirements", nonempty=False)
        require(set(required) <= requirements.keys(), f"{identity}: unknown requirement IDs")
        assigned_requirements.update(required)
        cases = strings(row.get("fixtures"), f"{identity}.fixtures", nonempty=False)
        require(set(cases) <= fixtures.keys(), f"{identity}: unknown fixture IDs")
        for field in ("sources", "tests"):
            paths = strings(row.get(field), f"{identity}.{field}", nonempty=field == "sources")
            for path in paths:
                require(path in sources, f"{identity}: unhashed evidence: {path}")
                used_sources.add(path)
        # Requirement locators and fixture/oracle hashes must also retain provenance.
        for selected, registry in ((required, requirements), (cases, fixtures)):
            for identifier in selected:
                relative = registry[identifier]
                require(relative in sources, f"{identity}: unhashed registry/manifest: {relative}")
                used_sources.add(relative)
    require(assigned_requirements == set(requirements), "unassigned requirements; update the capability inventory")
    candidates = data.get("feature_tags")
    if not isinstance(candidates, dict):
        raise ValueError("missing feature-tag review queue")
    discovered = feature_tags(root)
    require(set(candidates) == set(discovered), "feature-tag inventory drift; review additions/removals")
    for tag, locations in discovered.items():
        record = candidates[tag]
        require(isinstance(record, dict) and record.get("sources") == locations, f"{tag}: source locations changed")
        require(set(record) == {"sources", "review", "capability", "rationale", "evidence"}, f"{tag}: unexpected tag record fields")
        require(record.get("review") in REVIEWS, f"{tag}: unreviewed or invalid disposition")
        require(record.get("capability") in seen, f"{tag}: unknown capability owner")
        require(isinstance(record.get("rationale"), str) and record["rationale"].strip(), f"{tag}: missing review rationale")
        for path in locations:
            require(path in sources, f"{tag}: unhashed declaration: {path}")
            used_sources.add(path)
        evidence = record.get("evidence")
        if not isinstance(evidence, list) or not evidence:
            raise ValueError(f"{tag}: missing adjudication evidence")
        for item in evidence:
            require(isinstance(item, dict) and set(item) == {"path", "needle"}, f"{tag}: invalid evidence locator")
            path, needle = item["path"], item["needle"]
            require(path in sources, f"{tag}: unhashed call-site evidence: {path}")
            require(isinstance(needle, str) and needle.strip() and needle in local_file(root, path).read_text(encoding="utf-8"), f"{tag}: stale evidence locator: {path}")
            used_sources.add(path)
    inventory = data.get("api_inventory")
    if not isinstance(inventory, dict) or set(inventory) != set(engines):
        raise ValueError("incomplete baseline API inventory")
    for engine, record in inventory.items():
        require(isinstance(record, dict) and set(record) == {"sources", "scope", "operations"}, f"{engine}: invalid API inventory")
        require(isinstance(record["scope"], str) and record["scope"].strip(), f"{engine}: missing API review scope")
        for url in strings(record["sources"], f"{engine}.sources"):
            parsed = urlparse(url)
            require(parsed.scheme == "https" and parsed.netloc and not parsed.username and not parsed.password, f"{engine}: invalid public API source")
        operations = record["operations"]
        require(isinstance(operations, dict) and bool(operations) and set(operations) <= seen, f"{engine}: unassigned API operation")
        require(all(isinstance(value, str) and value.strip() for value in operations.values()), f"{engine}: missing API operation locator")
    require(used_sources == set(sources), "unreferenced source hashes in inventory")


def render(data) -> str:
    def cell(value: str) -> str:
        return value.replace("|", "\\|").replace("\r", " ").replace("\n", " ")
    lines = [
        "# Capability inventory — G00 review view", "",
        "Generated from [capabilities.json](../benchmarks/capabilities.json) by",
        "`uv run --locked --offline python tools/capabilities.py`. Edit the ledger, not this file.", "",
        f"Source audit: `{data['audited_revision']}`. G00 source inventory is complete for this revision.",
        "These are scoped source observations, not executed feature tests or a proof of full PDF coverage.",
        "Library includes lower-level crate APIs; each row states its remaining public integration.",
        "Linked tests/fixtures exist; listing them does not claim a fresh pass.",
        "All competitor family cells remain unprobed. Individual G01 probe results are separate:",
        "passing one fixture never promotes an entire family to supported or establishes parity.", "",
        "All literal tags have a manual disposition, every requirement ID has an owner, and",
        "discovered baseline API families are assigned below. New APIs, extensions, dynamic",
        "tags or hidden configurations reopen this inventory; unknown applicability stays open.", "",
        "Source hashes cover UTF-8 text with LF-normalized newlines for checkout portability;",
        "they are not executable/binary identities. Regeneration never refreshes evidence hashes.", "",
        "[Plan and acceptance gates](COMPETITIVE_PARITY.md) · [Capability summary](STATUS.md)", "",
        "## Source observations", "",
        "| ID / package | Feature | Library / CLI / host / viewer | Existing support | Remaining |",
        "|---|---|---|---|---|",
    ]
    for row in data["capabilities"]:
        states = " / ".join(row["surfaces"][key] for key in ("library", "cli", "host", "viewer"))
        lines.append(f"| `{row['id']}` / {row['package']} | {cell(row['feature'])} | {states} | {cell(row['implemented'])} | {cell(row['remaining'])} |")
    lines += ["", "## Baseline probes", "", "| Baseline | Unprobed capability cells |", "|---|---:|"]
    lines += [f"| {engine} | {len(data['capabilities'])} |" for engine in data["engines"]]
    lines += ["", "## Requirement ownership", "",
              f"All {len(set(identifier for row in data['capabilities'] for identifier in row['requirements']))} registered requirement IDs map to capability rows in the ledger.",
              "Ownership does not change normative verification status or certify the linked tests.", "",
              "## Baseline API discovery", "",
              "Documentation identifies operations to probe, not supported results. All six engines",
              "remain in every comparison denominator, including APIs without an adapter.", "",
              "| Baseline / sources | Review scope | Discovered API → capability |", "|---|---|---|"]
    for engine, record in data["api_inventory"].items():
        links = ", ".join(f"[source {index + 1}]({url})" for index, url in enumerate(record["sources"]))
        operations = "; ".join(f"{cell(api)} → `{identity}`" for identity, api in record["operations"].items())
        lines.append(f"| {engine}: {links} | {cell(record['scope'])} | {operations} |")
    counts = Counter(record["review"] for record in data["feature_tags"].values())
    lines += ["", "## Feature-tag dispositions", "",
              ", ".join(f"**{count} {kind}**" for kind, count in sorted(counts.items())) + ".", "",
              "`gap`: live unsupported subvariant; `boundary`: policy, invalid/unknown input or API",
              "boundary needing a concrete interoperability case; `dispatch`: handled in another",
              "layer; `dormant`: no live refusal caller found; `mixed`: multiple meanings.",
              "None is a competitor verdict. Boundaries stay assigned for G01 probes; dormant",
              "does not imply every related feature is implemented.", "",
              "This lexical scan finds literal `Feature::new` uses under crate `src` directories,",
              "excluding separate test files. Inline tests/comments may be included; macros, dynamic",
              "tags and untagged APIs require continuing manual review. The review inspected symbol",
              "uses, aliases and helper calls; the checker verifies locators and hashes, not reachability.", "",
              "| Tag / disposition | Owner | Review and evidence |", "|---|---|---|"]
    for tag, record in sorted(data["feature_tags"].items()):
        paths = dict.fromkeys(item["path"] for item in record["evidence"])
        sources = ", ".join(f"[{Path(path).name}](../{path})" for path in paths)
        lines.append(f"| `{tag}` / {record['review']} | `{record['capability']}` | {cell(record['rationale'])} {sources} |")
    return "\n".join(lines) + "\n"


def update_view(path: Path, expected: str, *, check: bool) -> None:
    if check:
        require(path.is_file() and path.read_text(encoding="utf-8") == expected, "stale capability view; run tools/capabilities.py")
    else:
        path.write_text(expected, encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="check the committed view without writing")
    args = parser.parse_args()
    try:
        data = read_json(ROOT / LEDGER)
        validate(data, ROOT)
        update_view(ROOT / VIEW, render(data), check=args.check)
    except (ValueError, KeyError, TypeError, OSError) as error:
        parser.exit(1, f"Capability inventory: {error}\n")
    print(f"Capability inventory valid: {len(data['capabilities'])} source observations, "
          f"{len(data['feature_tags'])} reviewed tags, "
          f"{len(data['capabilities']) * len(data['engines'])} unprobed family comparison cells.")


if __name__ == "__main__":
    main()
