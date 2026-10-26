//! Bounded plain text for Line captions, sharing the form font encoder.

use crate::features;
use crate::fonts::{Encoding, FontSource};
use crate::values::optional;
use pagura_core::{Diagnostics, Dictionary, Error, Meter, Name, Result, Value, text_string};
use pagura_document::Document;
use std::ops::Range;

pub(super) struct Row {
    pub(super) codes: Range<usize>,
    pub(super) width: f64,
}

pub(super) struct Text {
    pub(super) codes: Vec<u8>,
    pub(super) rows: Vec<Row>,
    pub(super) fonts: Dictionary,
    pub(super) ascent: f64,
    pub(super) descent: f64,
}

impl Text {
    pub(super) fn read(
        document: &Document,
        annotation: &Dictionary,
        fonts: &dyn FontSource,
    ) -> Result<Option<Self>> {
        if optional(document, annotation, b"RC")?.is_some() {
            return Err(Error::unsupported(
                features::ANNOTATION_APPEARANCE,
                "rich line caption generation is not implemented",
            ));
        }
        let Some(value) = optional(document, annotation, b"Contents")? else {
            return Ok(None);
        };
        let bytes = value
            .as_string()
            .ok_or(Error::syntax("line Contents must be a text string"))?;
        let limits = document.limits();
        let mut storage = Meter::new(limits.document_bytes, "caption storage budget exceeded");
        storage.charge(bytes.len().saturating_mul(3).min(limits.string_bytes))?;
        let mut diagnostics = Diagnostics::new(document.recovery());
        let text = text_string::decode(bytes, &limits, &mut diagnostics)?;
        document.report(diagnostics.take());
        if text.is_empty() {
            return Ok(None);
        }
        let mut dictionary = Dictionary::new();
        dictionary.insert(Name::new(b"Type"), Value::name(b"Font"));
        dictionary.insert(Name::new(b"Subtype"), Value::name(b"Type1"));
        dictionary.insert(Name::new(b"BaseFont"), Value::name(b"Helvetica"));
        dictionary.insert(Name::new(b"Encoding"), Value::name(b"WinAnsiEncoding"));
        let mut entries = Dictionary::new();
        entries.insert(Name::new(b"Caption"), Value::Dictionary(dictionary));
        let mut resources = Dictionary::new();
        resources.insert(Name::new(b"Font"), Value::Dictionary(entries.clone()));
        storage.charge(resources.retained_bytes())?;
        storage.charge(entries.retained_bytes())?;
        // The shared encoder bounds its fixed byte-to-Unicode map by this
        // conservative storage estimate; include it while text is prepared.
        storage.charge(256 * 64)?;
        let font = Encoding::load(document, &resources, b"Caption", fonts)?;
        let mut work = Meter::new(limits.operations, "caption layout work budget exceeded");
        work.charge(256)?; // The shared reversible font map visits every byte code.
        let mut characters = Meter::new(limits.value_nodes, "caption character budget exceeded");
        let mut rows = Vec::new();
        let mut codes = Vec::new();
        let (mut start, mut width) = (0, 0.0);
        let mut input = text.chars().peekable();
        while let Some(c) = input.next() {
            work.charge(1)?;
            characters.charge(1)?;
            if matches!(c, '\r' | '\n') {
                if c == '\r' && input.next_if_eq(&'\n').is_some() {
                    work.charge(1)?;
                    characters.charge(1)?;
                }
                storage.reserve_capacity(&mut rows, 1)?;
                rows.push(Row {
                    codes: start..codes.len(),
                    width,
                });
                start = codes.len();
                width = 0.0;
            } else {
                let code = font.encode(if c == '\t' { ' ' } else { c })?;
                width += code.advance;
                if !width.is_finite() {
                    return Err(Error::limit("caption width exceeds numeric range"));
                }
                storage.reserve_capacity(&mut codes, 1)?;
                codes.push(code.code);
            }
        }
        storage.reserve_capacity(&mut rows, 1)?;
        rows.push(Row {
            codes: start..codes.len(),
            width,
        });
        Ok(Some(Self {
            codes,
            rows,
            fonts: entries,
            ascent: font.ascent,
            descent: font.descent,
        }))
    }
}
