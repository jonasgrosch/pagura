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

Full rewrite, object deletion, undo/redo, encryption-on-write and viewer editing
controls remain outside these foundations. See [ADR 0031](adr/0031-editing-foundations.md)
for ownership, budgets, pending dependency review and validation evidence.
