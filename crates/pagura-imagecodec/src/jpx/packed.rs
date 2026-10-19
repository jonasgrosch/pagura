//! Packed packet headers, T.800 A.7.4 (PPM) and A.7.5 (PPT).
//!
//! The marker segments carry fragments of one header stream, ordered by
//! their index rather than by their place in the codestream. Fragments are
//! kept as cursors over the codestream itself, so every error reports where
//! the byte really is. A packet header never continues from one fragment
//! into the next, so each header is read from a cursor over one fragment.

use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Position, Result};

use crate::budget::Budget;

use super::syntax;

/// The truncation message of a fragment cursor: running off its end means a
/// header crosses into the next fragment, or the headers end too early.
const FRAGMENT_END: &str = "JPEG 2000 packet header runs past its packed headers";

/// PPM or PPT fragments, in index order.
#[derive(Default)]
pub(super) struct Fragments<'a> {
    indices: Vec<u8>,
    cursors: Vec<Cursor<'a>>,
}

impl<'a> Fragments<'a> {
    pub(super) fn present(&self) -> bool {
        !self.indices.is_empty()
    }

    /// Adds the fragment that `fields`, the parameters of a PPM or PPT
    /// marker segment, holds after its index.
    pub(super) fn push(&mut self, fields: &mut Cursor<'a>, budget: &mut Budget) -> Result<()> {
        let at = fields.position();
        let index = fields.u8()?;
        let Err(slot) = self.indices.binary_search(&index) else {
            return Err(syntax("duplicate JPEG 2000 packed-header index", at));
        };
        if fields.at_end() {
            return Err(fields.error("empty JPEG 2000 packed-header fragment"));
        }
        let fragment = fields
            .take_cursor(fields.remaining())?
            .with_message(FRAGMENT_END);
        budget.work(self.indices.len() - slot)?;
        budget.push(&mut self.indices, index)?;
        self.indices[slot..].rotate_right(1);
        budget.push(&mut self.cursors, fragment)?;
        self.cursors[slot..].rotate_right(1);
        Ok(())
    }

    /// The headers of the tile-part whose header held these PPT fragments.
    pub(super) fn headers(&self, budget: &mut Budget) -> Result<Option<Headers<'a>>> {
        if !self.present() {
            return Ok(None);
        }
        let mut pieces = budget.with_capacity(self.cursors.len())?;
        pieces.extend(self.cursors.iter().cloned());
        Ok(Some(Headers { pieces, current: 0 }))
    }

    /// The fragment holding the next unread byte, if any.
    fn current(&mut self) -> Option<&mut Cursor<'a>> {
        self.cursors.iter_mut().find(|fragment| !fragment.at_end())
    }

    /// Where the fragments end.
    fn end(&self) -> Position {
        self.cursors
            .last()
            .map_or(Position::Unknown, Cursor::position)
    }
}

/// The PPM fragments of the main header, shared by all tile-parts in turn.
#[derive(Default)]
pub(super) struct Ppm<'a> {
    pub(super) fragments: Fragments<'a>,
}

impl<'a> Ppm<'a> {
    /// The headers of the next tile-part: `Nppm` bytes after their length
    /// field `Nppm`, which lies within one fragment while the headers may
    /// continue into later ones.
    pub(super) fn next(&mut self, budget: &mut Budget) -> Result<Headers<'a>> {
        let end = self.fragments.end();
        let fragment = self.fragments.current();
        let Some(fragment) = fragment.filter(|fragment| fragment.remaining() >= 4) else {
            return Err(syntax("split or truncated JPEG 2000 Nppm field", end));
        };
        let mut length = u32_to_usize(fragment.u32()?);
        let mut pieces = Vec::new();
        while length != 0 {
            let fragment = self
                .fragments
                .current()
                .ok_or_else(|| syntax("truncated JPEG 2000 PPM headers", end))?;
            let piece = fragment.take_cursor(length.min(fragment.remaining()))?;
            length -= piece.remaining();
            budget.push(&mut pieces, piece)?;
        }
        Ok(Headers { pieces, current: 0 })
    }

    /// Rejects headers left over once every tile-part has been read.
    pub(super) fn finish(&mut self) -> Result<()> {
        if let Some(fragment) = self.fragments.current() {
            return Err(fragment.error("unused JPEG 2000 PPM tile-part headers"));
        }
        Ok(())
    }
}

/// The packed headers of one tile-part, in pieces that each lie within one
/// fragment.
pub(super) struct Headers<'a> {
    pieces: Vec<Cursor<'a>>,
    current: usize,
}

impl<'a> Headers<'a> {
    /// The number of unread bytes.
    pub(super) fn remaining(&self) -> usize {
        self.pieces
            .get(self.current..)
            .unwrap_or_default()
            .iter()
            .map(Cursor::remaining)
            .sum()
    }

    /// Reads the next packet header with `read`, from a cursor over the rest
    /// of the fragment it starts in.
    pub(super) fn header<T>(
        &mut self,
        read: impl FnOnce(&mut Cursor<'a>) -> Result<T>,
    ) -> Result<T> {
        while self.pieces.get(self.current).is_some_and(Cursor::at_end) {
            self.current += 1;
        }
        let end = self
            .pieces
            .last()
            .map_or(Position::Unknown, Cursor::position);
        let Some(piece) = self.pieces.get_mut(self.current) else {
            return Err(Error::syntax(FRAGMENT_END).at(end));
        };
        match read(piece) {
            // T.800 A.7.4, A.7.5: a header that needs bytes past its fragment is
            // split, whatever the next fragment holds.
            Err(error) if error.message == FRAGMENT_END => {
                Err(self.pieces.get(self.current + 1).map_or(error, |next| {
                    syntax(
                        "JPEG 2000 packed marker splits a packet header",
                        next.position(),
                    )
                }))
            }
            result => result,
        }
    }
}
