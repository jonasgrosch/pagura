//! The bytes of one content sequence and the positions within them (7.8.2).

use pagura_core::{Error, Limits, ObjectId, Position, Result};
use std::borrow::Cow;

/// The content of a page or form: its streams' decoded data concatenated
/// into one token sequence (7.8.2), with the start of each stream so that
/// every offset maps back to a position in its stream.
///
/// A page's `Contents` array may split any token or operator between
/// streams; each stream is followed by one line feed, which separates the
/// last token of a stream from the first of the next.
#[derive(Debug, Clone)]
pub struct Content<'a> {
    bytes: Cow<'a, [u8]>,
    /// The offset at which each stream starts, increasing, and its object.
    origins: Vec<(usize, ObjectId)>,
    /// Whether damaged filter data cut the sequence short at its end.
    cut: bool,
}

impl Content<'static> {
    /// Concatenates the decoded data of `streams` in order, each followed by
    /// a line feed.
    ///
    /// # Errors
    ///
    /// Returns a `Limit` error when the concatenation exceeds
    /// `limits.decoded_bytes` or cannot be allocated.
    pub fn from_streams<'s>(
        streams: impl IntoIterator<Item = (ObjectId, Cow<'s, [u8]>)>,
        limits: &Limits,
    ) -> Result<Self> {
        let mut bytes = Vec::new();
        let mut origins = Vec::new();
        for (object, data) in streams {
            let end = bytes
                .len()
                .checked_add(data.len())
                .and_then(|n| n.checked_add(1))
                .filter(|&n| n <= limits.decoded_bytes)
                .ok_or(Error::limit("content sequence byte budget exceeded"))?;
            bytes
                .try_reserve(end - bytes.len())
                .map_err(|_| Error::limit("content sequence allocation failed"))?;
            origins
                .try_reserve(1)
                .map_err(|_| Error::limit("content sequence allocation failed"))?;
            origins.push((bytes.len(), object));
            bytes.extend_from_slice(&data);
            bytes.push(b'\n');
        }
        Ok(Self {
            bytes: Cow::Owned(bytes),
            origins,
            cut: false,
        })
    }

    /// Marks the last stream's data as cut short by damaged filter data
    /// (`filter.corrupt`): the sequence ends there, and a token that reaches
    /// the end may be incomplete, so the line feed that would end it is
    /// removed.
    #[must_use]
    pub fn cut_short(mut self) -> Self {
        if !self.cut && !self.origins.is_empty() {
            self.bytes.to_mut().pop();
        }
        self.cut = true;
        self
    }
}

impl<'a> Content<'a> {
    /// Content that is not a stream of the document, such as a generated
    /// appearance: offsets are buffer positions.
    pub fn from_bytes(bytes: impl Into<Cow<'a, [u8]>>) -> Self {
        Self {
            bytes: bytes.into(),
            origins: Vec::new(),
            cut: false,
        }
    }

    /// Whether damaged filter data cut the sequence short at its end.
    pub fn is_cut(&self) -> bool {
        self.cut
    }

    /// The concatenated bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The stream holding `offset` and the offset within it.
    fn origin(&self, offset: usize) -> Option<(ObjectId, usize)> {
        let count = self.origins.partition_point(|&(start, _)| start <= offset);
        let (start, object) = *self.origins.get(count.checked_sub(1)?)?;
        Some((object, offset - start))
    }

    /// The position of `offset`: a stream position, or a buffer position for
    /// content that is not a stream.
    pub fn position(&self, offset: usize) -> Position {
        self.origin(offset)
            .map_or(Position::Buffer(offset), |(object, offset)| {
                Position::Stream { object, offset }
            })
    }

    /// The offset of the last byte, where an unfinished sequence is
    /// reported.
    pub fn end(&self) -> usize {
        self.bytes.len().saturating_sub(1)
    }

    /// Places `error` in this content. An error without a position is placed
    /// at `offset`, the start of the token being processed; one with a buffer
    /// position is placed at that offset, which then names a stream and
    /// records it as the object. Stream and file positions are kept.
    #[must_use]
    pub fn locate(&self, mut error: Error, offset: usize) -> Error {
        let offset = match error.position {
            Position::Buffer(n) => n,
            _ => offset,
        };
        if matches!(error.position, Position::Unknown | Position::Buffer(_))
            && let Some((object, offset)) = self.origin(offset)
        {
            error.position = Position::Stream { object, offset };
            error = error.in_object(object);
        } else if error.position == Position::Unknown {
            error.position = Position::Buffer(offset);
        }
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two() -> Content<'static> {
        Content::from_streams(
            [
                (ObjectId::new(4, 0), Cow::Borrowed(b"q 1 0".as_slice())),
                (ObjectId::new(7, 0), Cow::Borrowed(b"0 1 0 0 cm".as_slice())),
            ],
            &Limits::default(),
        )
        .unwrap()
    }

    #[test]
    fn streams_concatenate_with_line_feeds_and_positions_name_their_stream() {
        let content = two();
        assert_eq!(content.bytes(), b"q 1 0\n0 1 0 0 cm\n");
        assert_eq!(
            content.position(2),
            Position::Stream {
                object: ObjectId::new(4, 0),
                offset: 2
            }
        );
        assert_eq!(
            content.position(6),
            Position::Stream {
                object: ObjectId::new(7, 0),
                offset: 0
            }
        );
        assert_eq!(
            content.position(content.end()),
            Position::Stream {
                object: ObjectId::new(7, 0),
                offset: 10
            }
        );
        assert_eq!(
            Content::from_bytes(b"q".as_slice()).position(0),
            Position::Buffer(0)
        );
    }

    #[test]
    fn located_errors_keep_inner_positions_and_map_buffer_offsets() {
        let content = two();
        let error = content.locate(Error::syntax("x"), 8);
        assert_eq!(
            (error.position, error.object),
            (
                Position::Stream {
                    object: ObjectId::new(7, 0),
                    offset: 2
                },
                Some(ObjectId::new(7, 0))
            )
        );
        let error = content.locate(Error::syntax("x").at(Position::Buffer(1)), 8);
        assert_eq!(
            error.position,
            Position::Stream {
                object: ObjectId::new(4, 0),
                offset: 1
            }
        );
        let inner = Position::Stream {
            object: ObjectId::new(9, 0),
            offset: 3,
        };
        assert_eq!(
            content.locate(Error::syntax("x").at(inner), 8).position,
            inner
        );
        let buffer = Content::from_bytes(b"abc".as_slice());
        assert_eq!(
            buffer.locate(Error::syntax("x"), 2).position,
            Position::Buffer(2)
        );
    }

    #[test]
    fn the_concatenation_is_budgeted() {
        let limits = Limits {
            decoded_bytes: 10,
            ..Limits::default()
        };
        let error = Content::from_streams(
            [(ObjectId::new(1, 0), Cow::Borrowed(b"0123456789".as_slice()))],
            &limits,
        )
        .unwrap_err();
        assert_eq!(error.kind, pagura_core::ErrorKind::Limit);
    }
}
