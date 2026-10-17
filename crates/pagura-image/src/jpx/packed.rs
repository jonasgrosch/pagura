//! PPM/PPT assembly, retaining source positions across marker fragments.

use pagura_core::Result;

use super::io::{Budget, Reader, Span, syntax, usize_from};

#[derive(Default)]
pub(super) struct Fragments {
    bytes: Vec<u8>,
    spans: Vec<Span>,
    next_index: u16,
    first_index: u16,
}

impl Fragments {
    pub fn with_index(index: u16) -> Self {
        Self {
            next_index: index,
            first_index: index,
            ..Self::default()
        }
    }

    pub fn end_index(&self) -> u16 {
        self.next_index
    }

    pub fn present(&self) -> bool {
        self.next_index != self.first_index
    }

    pub fn push(&mut self, fields: &mut Reader<'_>, budget: &mut Budget<'_>) -> Result<()> {
        if u16::from(fields.u8()?) != self.next_index {
            return Err(syntax(
                "out-of-order JPEG 2000 packed-header marker",
                fields.position() - 1,
            ));
        }
        self.next_index += 1;
        if fields.remaining() != 0 {
            budget.push(
                &mut self.spans,
                Span {
                    logical: self.bytes.len(),
                    source: fields.position(),
                },
            )?;
            budget.append(&mut self.bytes, fields.take(fields.remaining())?)?;
        }
        Ok(())
    }

    pub fn reader(&self) -> Option<Reader<'_>> {
        self.present()
            .then(|| Reader::mapped(&self.bytes, 0, &self.spans))
    }
}

#[derive(Default)]
pub(super) struct Ppm {
    pub fragments: Fragments,
    offset: usize,
}

impl Ppm {
    pub fn next(&mut self) -> Result<Reader<'_>> {
        let mut fields = Reader::mapped(
            &self.fragments.bytes[self.offset..],
            self.offset,
            &self.fragments.spans,
        );
        // Nppm is a complete field within one marker; Ippm may span markers.
        let next_span = self
            .fragments
            .spans
            .partition_point(|s| s.logical <= self.offset);
        let span_end = self
            .fragments
            .spans
            .get(next_span)
            .map_or(self.fragments.bytes.len(), |s| s.logical);
        if span_end - self.offset < 4 {
            return Err(syntax(
                "split or truncated JPEG 2000 Nppm field",
                fields.position(),
            ));
        }
        let length = usize_from(fields.u32()?)?;
        let start = self.offset + 4;
        let bytes = fields.take(length)?;
        self.offset = start + length;
        Ok(Reader::mapped(bytes, start, &self.fragments.spans))
    }

    pub fn finish(&self) -> Result<()> {
        if self.offset != self.fragments.bytes.len() {
            return Err(syntax(
                "unused JPEG 2000 PPM tile-part headers",
                self.fragments.spans.first().map_or(0, |s| s.source),
            ));
        }
        Ok(())
    }
}
