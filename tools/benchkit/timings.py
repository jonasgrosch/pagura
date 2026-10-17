"""Validation of monotonic adapter timing records."""
import json


def parse_stages(diagnostic, page, elapsed_ns):
    """Validate the single-page subset without relaxing document timing checks."""
    result = parse_document_stages(diagnostic, [page], elapsed_ns)
    if result is not None:
        result.pop("pages")
    return result


def parse_document_stages(diagnostic, selected_pages, elapsed_ns):
    """Validate exactly the selected ordered pages, one open, and global accounting."""
    def unique_object(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate timing field")
            result[key] = value
        return result

    records = []
    for line in diagnostic.splitlines():
        if not line.startswith("{"):
            continue
        try:
            record = json.loads(line, object_pairs_hook=unique_object)
        except ValueError as error:
            raise ValueError("malformed JSON timing record") from error
        if isinstance(record, dict) and ("open_ns" in record or "pages" in record):
            records.append(record)
    if not records:
        return None
    if len(records) != 1:
        raise ValueError("ambiguous timing records")
    record = records[0]
    pages = record.get("pages")
    if type(record.get("schema")) is not int or record["schema"] != 1 or not isinstance(pages, list):
        raise ValueError("invalid timing schema or page count")
    if (not selected_pages or len(pages) != len(selected_pages)
            or any(type(page) is not int or page < 0 for page in selected_pages)
            or len(set(selected_pages)) != len(selected_pages)):
        raise ValueError("timing page count mismatch")
    for actual, expected in zip(pages, selected_pages):
        if not isinstance(actual, dict) or type(actual.get("page")) is not int or actual["page"] != expected:
            raise ValueError("timing page mismatch")
    values = [record.get("open_ns"), record.get("total_ns")]
    values.extend(value for page in pages for value in (page.get("render_ns"), page.get("emit_ns")))
    if any(type(value) is not int or value < 0 for value in values):
        raise ValueError("stage times must be nonnegative integer nanoseconds")
    opened, total = values[:2]
    rendered = sum(page["render_ns"] for page in pages)
    emitted = sum(page["emit_ns"] for page in pages)
    if opened + rendered + emitted > total or total > elapsed_ns:
        raise ValueError("stage times exceed total or process time")
    return {"open_ns": opened, "render_ns": rendered, "emit_ns": emitted,
            "engine_ns": opened + rendered, "total_ns": total, "pages": pages}
