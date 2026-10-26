//! Page annotation attachment JSON, separate from catalog name-tree ordering.

use super::{hex, object, specification};
use crate::{failure::Failure, json};
use pagura::{Document, Name};
use std::io::Write;

pub(super) fn list(document: &Document, page: usize, out: &mut impl Write) -> Result<(), Failure> {
    let annotations = document.page(page)?.attachment_annotations(document)?;
    write!(out, "{{\"schema\":1,\"page\":{page},\"annotations\":[")?;
    for (index, annotation) in annotations.iter().enumerate() {
        if index > 0 {
            out.write_all(b",")?;
        }
        write!(out, "{{\"index\":{},\"object\":", annotation.index)?;
        object(out, Some(annotation.object))?;
        out.write_all(b",\"rectangle\":")?;
        json::rectangle(out, annotation.rectangle)?;
        out.write_all(b",\"contents\":")?;
        json::optional(out, annotation.contents.as_deref())?;
        out.write_all(b",\"icon_hex\":")?;
        hex(out, Some(annotation.icon_name()))?;
        out.write_all(b",\"declared_icon_hex\":")?;
        hex(out, annotation.icon.map(Name::as_bytes))?;
        out.write_all(b",\"file\":{\"object\":")?;
        object(out, annotation.specification.object)?;
        specification(out, &annotation.specification)?;
        out.write_all(b"}}")?;
    }
    out.write_all(b"],")?;
    json::diagnostics(out, document)?;
    writeln!(out, "}}")?;
    Ok(())
}
