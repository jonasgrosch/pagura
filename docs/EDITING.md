# Editing and saving forms

The library exposes `pagura::editing::{Transaction, save_filled_form, SaveOptions}`.
Open input with `Recovery::Strict`; modified incremental saves reject repair-mode
and encrypted sources. The default signature policy rejects signed or uncertain
input. `SignaturePolicy::PreserveSignedBytes` explicitly permits appending to a
signed source but promises only original-byte preservation, not trust, continued
validity or permission under DocMDP/FieldMDP.

```rust
use pagura::{Document, Error, OpenOptions, Recovery, Result};
use pagura::editing::{SaveOptions, save_filled_form};
use pagura::forms::FillValue;

fn fill(input: Vec<u8>, field_index: usize, value: &str) -> Result<Vec<u8>> {
    let document = Document::open(input, &OpenOptions {
        recovery: Recovery::Strict,
        ..Default::default()
    })?;
    let mut filler = document.form_filler()?
        .ok_or(Error::invalid_argument("document has no form"))?;
    filler.set(field_index, FillValue::Text(value))?;
    save_filled_form(&filler, &SaveOptions::default())
}
```

Field indexes come from the filler form's typed field list. Text/choice values
and normal appearances save together; existing checkbox/radio state dictionaries
are retained. Default values, rollover/down appearances and unrelated dictionary
entries survive. The existing appearance generator's font and feature limits
apply. A failed save returns no partial output and leaves the filler available
for correction. Password changes, nested direct field/widget identities and
buttons requiring generated state dictionaries return explicit unsupported tags.

For lower-level edits, start `Transaction::new(&document)`, use
`edit_dictionary(id, closure)`, `replace(id, object)` or `add(object)`, and call
`commit(&SaveOptions::default())`. Replacement keeps the original generation;
allocation never reuses free object numbers. References resolve against the
source plus staged objects at commit. `reset(id)` removes one staged object;
`rollback()` or dropping the transaction discards it. Schema correctness of
arbitrary low-level replacements is the caller's responsibility.

Commit returns the complete new file, beginning with the exact original bytes.
Reopen it to create the next immutable snapshot. Commit can be retried and a
no-op returns the original bytes exactly. The engine performs no filesystem IO;
hosts choose their own atomic file replacement/download behavior. Historical
values remain in the file: incremental editing is not redaction.

Full rewrite, object deletion, undo/redo and encryption-on-write remain outside
these foundations. See [ADR 0031](adr/0031-editing-foundations.md)
for ownership, budgets, pending dependency review and validation evidence.

## Try in the browser

Serve the viewer with `python3 tools/serve_web.py` after obtaining a WASI host
built with `--features bundled-data` (the editing CI publishes this artifact).
Open `http://localhost:8000/?demo=form` for the sample, or select your own
fillable PDF. Click the highlighted fields directly on the page: type into text
fields, select choices, and toggle checkbox/radio widgets. **Save PDF** applies
pending edits, previews the saved document and downloads it in one action.
**Discard changes** restores the last saved values. **Fill forms** toggles the
interactive overlay without discarding drafts. Reopen the download to check
persistence. Original files are untouched and no PDF is uploaded.

Normal display uses reported compatibility repairs. **Strict rendering** is an
opt-in diagnostic mode. Form metadata and saves use a separate strict snapshot
of the original bytes, so tolerant display does not relax structural save checks.
Sources requiring structural repairs still cannot be edited. Content-only repairs
such as a numeric sc used in an ICC colour space can display compatibly while
unrelated form changes append without rewriting the original content stream.

The overlay follows CropBox, page Rotate, UserUnit and zoom; drafts survive page
virtualization. Optional-content, NoZoom/NoRotate, ToggleNoView and rotated-MK
widgets are omitted pending their dedicated interaction semantics. General PDF
text editing, undo/redo, password/rich-text fields and signature editing remain
unsupported. Signed/encrypted sources retain the library's refusal policy.

For duplicated widget dictionaries, the browser can reconcile page references
only when the complete dictionary matches exactly one form-tree widget. This
happens during a changed save, preserves original bytes, and refuses ambiguous
matches. Matching a name or rectangle alone is never sufficient.
