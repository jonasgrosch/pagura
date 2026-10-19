//! Source-backed recovery of direct streams (7.3.8.1).

use super::Parser;
use crate::{Dictionary, Error, Meter, Name, Result};
use std::ops::Range;

/// One step from a parsed value to a nested dictionary or array element.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PathComponent {
    /// A decoded dictionary key.
    Key(Name),
    /// An array index.
    Index(usize),
}

impl PathComponent {
    /// Estimated retained storage, including heap name bytes when needed.
    pub fn retained_bytes(&self) -> usize {
        size_of::<Self>().saturating_add(match self {
            Self::Key(name) => name.heap_bytes(),
            Self::Index(_) => 0,
        })
    }
}

/// An illegal direct stream recovered without changing the parsed value tree.
/// Its dictionary remains at `path`; payload bytes remain in the input slice.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DirectStream {
    /// Path to its dictionary in the parsed value.
    pub path: Vec<PathComponent>,
    /// Dictionary through the complete endstream token, relative to parser input.
    pub source: Range<usize>,
    /// Encoded payload, relative to parser input.
    pub data: Range<usize>,
}

/// Payload consumer used by [`Parser::parse_with_streams`].
/// It starts before `stream` and ends after a verified `endstream` token.
pub type DirectStreamHandler<'a, 'h> =
    dyn FnMut(&mut Parser<'a>, &Dictionary) -> Result<Option<Range<usize>>> + 'h;

/// Direct-stream recovery state for one [`Parser::parse_with_streams`] call.
/// The handler borrow (`'b`) is independent of what the handler captures
/// (`'h`), so a caller's closure is not forced to outlive its borrow.
pub(super) struct Context<'a, 'b, 'h> {
    pub(super) handler: &'b mut DirectStreamHandler<'a, 'h>,
    pub(super) path: Vec<PathComponent>,
    pub(super) found: Vec<DirectStream>,
    pub(super) count: usize,
    metadata: Meter,
}

impl<'a, 'b, 'h> Context<'a, 'b, 'h> {
    pub(super) fn new(handler: &'b mut DirectStreamHandler<'a, 'h>, budget: usize) -> Self {
        Self {
            handler,
            path: Vec::new(),
            found: Vec::new(),
            count: 0,
            metadata: Meter::new(budget, "direct stream metadata budget exceeded"),
        }
    }

    /// Retains a descriptor for the stream whose dictionary is at the current
    /// path. Descriptor growth and the copied path are charged to
    /// [`Limits::document_bytes`](crate::Limits::document_bytes) before anything is copied.
    pub(super) fn record(&mut self, source: Range<usize>, data: Range<usize>) -> Result<()> {
        let capacity = if self.found.len() == self.found.capacity() {
            self.found.capacity().saturating_mul(2).max(1)
        } else {
            self.found.capacity()
        };
        let growth = capacity
            .saturating_sub(self.found.capacity())
            .saturating_mul(size_of::<DirectStream>());
        let bytes = self.path.iter().fold(growth, |sum, part| {
            sum.saturating_add(part.retained_bytes())
        });
        self.metadata.charge(bytes)?;
        self.found
            .try_reserve_exact(capacity.saturating_sub(self.found.len()))
            .map_err(|_| Error::limit("direct stream metadata allocation failed"))?;
        self.found.push(DirectStream {
            path: self.path.clone(),
            source,
            data,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::PathComponent;
    use crate::repairs::DIRECT_STREAM;
    use crate::syntax::{Origin, Parser};
    use crate::{Limits, Name, Recovery};

    #[test]
    fn direct_streams_are_opt_in_positioned_and_separate_from_dictionary_values() {
        let bytes = b"<< /Contents << /Length 1 >> stream\nZ\nendstream >>";
        let mut normal =
            Parser::new(bytes, Origin::File, Limits::default()).with_recovery(Recovery::Repair);
        assert!(normal.parse().is_err());
        let mut strict = Parser::new(bytes, Origin::File, Limits::default());
        let error = strict
            .parse_with_streams(&mut |_, _| panic!("strict mode must reject before the callback"))
            .unwrap_err();
        assert_eq!(error.repair, Some(DIRECT_STREAM));
        let mut parser =
            Parser::new(bytes, Origin::File, Limits::default()).with_recovery(Recovery::Repair);
        let (value, streams) = parser
            .parse_with_streams(&mut |parser, _| {
                assert!(parser.keyword(b"stream"));
                let start = parser.offset() + 1;
                parser.seek(start + 1)?;
                assert!(parser.keyword(b"endstream"));
                Ok(Some(start..start + 1))
            })
            .unwrap();
        assert_eq!(streams.len(), 1);
        assert_eq!(
            streams[0].path,
            [PathComponent::Key(Name::new(b"Contents"))]
        );
        assert_eq!(&bytes[streams[0].data.clone()], b"Z");
        assert!(
            value
                .as_dictionary()
                .unwrap()
                .get(b"Contents")
                .unwrap()
                .as_dictionary()
                .is_some()
        );
        assert_eq!(parser.take_diagnostics()[0].repair(), Some(DIRECT_STREAM));
    }
    #[test]
    fn descriptor_growth_is_bounded_before_copying_long_source_paths() {
        let mut bytes = b"<< /".to_vec();
        bytes.extend_from_slice(&[b'A'; 256]);
        bytes.extend_from_slice(b" [");
        for _ in 0..3 {
            bytes.extend_from_slice(b"<< /Length 1 >> stream\nZ\nendstream ");
        }
        bytes.extend_from_slice(b"] >>");
        let mut parser = Parser::new(
            &bytes,
            Origin::File,
            Limits {
                document_bytes: 512,
                ..Limits::default()
            },
        )
        .with_recovery(Recovery::Repair);
        let error = parser
            .parse_with_streams(&mut |parser, _| {
                assert!(parser.keyword(b"stream"));
                let start = parser.offset() + 1;
                parser.seek(start + 1)?;
                assert!(parser.keyword(b"endstream"));
                Ok(Some(start..start + 1))
            })
            .unwrap_err();
        assert_eq!(error.kind, crate::ErrorKind::Limit);
        assert_eq!(error.message, "direct stream metadata budget exceeded");
    }
}
