//! Static Type 1 font dictionaries, PFA/PFB framing and cipher decoding.
//!
//! Follows Adobe Type 1 Font Format, which 9.9 references. This is a font
//! reader, without a PostScript execution environment.
use crate::budget::Work;
use crate::cff::strings;
use crate::memory::{encoding, names, vector};
use crate::read::{self, glyph_id, syntax, unsupported};
use crate::{GlyphId, features};
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Name, Position, Result};
use pagura_raster::{Path, Transform};
use std::borrow::Cow;
use std::collections::BTreeMap;

mod aliases;
mod charstring;
mod charstrings;
mod cipher;
mod tokens;

use aliases::{Aliases, Assignment};
use charstrings::CharStrings;
use cipher::{CHARSTRING_KEY, EEXEC_DISCARD, EEXEC_KEY, decrypt};
use tokens::Tokens;

/// PFB framing (Adobe TN 5040): each segment starts with a marker byte and
/// a kind; ASCII and binary segments carry a 32-bit little-endian length.
const PFB_MARKER: u8 = 128;
const PFB_ASCII: u8 = 1;
const PFB_BINARY: u8 = 2;
const PFB_END: u8 = 3;
/// The only FontType of a Type 1 font program (Type 1 5.1).
const FONT_TYPE: usize = 1;
/// The default charstring lenIV (Type 1 5.1, Private dictionary).
const DEFAULT_LEN_IV: usize = 4;

/// A decrypted charstring and the plaintext offset of its first byte.
#[derive(Debug)]
struct Charstring {
    bytes: Vec<u8>,
    at: usize,
}

impl Charstring {
    fn cursor(&self) -> Cursor<'_> {
        read::cursor(&self.bytes).with_base(self.at)
    }

    const fn end(&self) -> usize {
        self.at.saturating_add(self.bytes.len())
    }
}

#[derive(Debug)]
pub(crate) struct Type1 {
    glyphs: Vec<Charstring>,
    glyph_count: u32,
    subrs: Vec<Charstring>,
    names: BTreeMap<Name, GlyphId>,
    glyph_names: Vec<Option<Name>>,
    encoding: Box<[Option<Name>; 256]>,
    matrix: Transform,
}

impl Type1 {
    pub(crate) fn parse(data: &[u8], work: &mut Work<'_>) -> Result<Self> {
        let (flat, binary) = flatten(data, work)?;
        let mut clear = cleartext(&flat, work)?;
        let plain = eexec(&flat, &clear, binary, work)?;
        let aliases = std::mem::take(&mut clear.aliases);
        let Private { subrs, charstrings } = private(&plain, aliases, work)?;
        let glyph_names = crate::names::indexed(
            charstrings.glyphs.len(),
            charstrings
                .names
                .iter()
                .map(|(name, &glyph)| Ok((glyph, name))),
            work,
            Position::Buffer(clear.start),
        )?;
        Ok(Self {
            glyph_count: glyph_id(charstrings.glyphs.len(), Position::Unknown)?.0,
            glyphs: charstrings.glyphs,
            subrs,
            names: charstrings.names,
            glyph_names,
            encoding: clear.encoding,
            matrix: clear.matrix,
        })
    }

    pub(crate) const fn glyph_count(&self) -> u32 {
        self.glyph_count
    }

    pub(crate) const fn matrix(&self) -> Transform {
        self.matrix
    }

    pub(crate) fn glyph_name(&self, glyph: GlyphId) -> Option<&Name> {
        crate::names::get(&self.glyph_names, glyph)
    }

    pub(crate) fn name(&self, name: &[u8]) -> Option<GlyphId> {
        self.names.get(name).copied()
    }

    pub(crate) fn encoding(&self) -> &[Option<Name>; 256] {
        &self.encoding
    }

    pub(crate) fn outline(&self, glyph: GlyphId, work: &mut Work<'_>) -> Result<Path> {
        charstring::outline(self, glyph, work)
    }

    /// The charstring width, if the glyph interprets.
    pub(crate) fn advance(&self, glyph: GlyphId, work: &mut Work<'_>) -> Option<f64> {
        charstring::advance(self, glyph, work).ok()
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        charstrings(&self.glyphs)
            .saturating_add(charstrings(&self.subrs))
            .saturating_add(names(&self.names))
            .saturating_add(crate::names::heap_bytes(&self.glyph_names))
            .saturating_add(encoding(&self.encoding))
    }
}

/// Type 1 keeps decrypted charstrings and subroutines, not decrypted
/// dictionaries.
fn charstrings(values: &Vec<Charstring>) -> usize {
    values.iter().fold(vector(values), |sum, c| {
        sum.saturating_add(vector(&c.bytes))
    })
}

/// A charstring's bytes, decrypted unless lenIV is -1; `start` is their
/// plaintext offset.
fn charstring_bytes(bytes: &[u8], start: usize, len_iv: Option<usize>) -> Result<Charstring> {
    let Some(discard) = len_iv else {
        return Ok(Charstring {
            bytes: bytes.to_vec(),
            at: start,
        });
    };
    Ok(Charstring {
        bytes: decrypt(bytes, CHARSTRING_KEY, discard, Position::Buffer(start))?,
        at: start.saturating_add(discard),
    })
}

/// The program with PFB framing removed, and whether a segment was binary.
fn flatten<'a>(data: &'a [u8], work: &mut Work<'_>) -> Result<(Cow<'a, [u8]>, bool)> {
    if data.first() != Some(&PFB_MARKER) {
        return Ok((Cow::Borrowed(data), false));
    }
    let mut r = read::cursor(data);
    let mut result = Vec::new();
    let mut binary = false;
    loop {
        let at = r.position();
        work.work(1, at)?;
        if r.u8().ok() != Some(PFB_MARKER) {
            return Err(syntax(at, "invalid PFB segment marker"));
        }
        let kind = r.u8().map_err(|_| syntax(at, "truncated PFB header"))?;
        if kind == PFB_END {
            if !r.at_end() {
                return Err(r.error("trailing PFB data"));
            }
            break;
        }
        if !matches!(kind, PFB_ASCII | PFB_BINARY) {
            return Err(r.error("invalid PFB segment kind"));
        }
        binary |= kind == PFB_BINARY;
        let size = u32_to_usize(u32::from_le_bytes(r.take_array()?));
        work.decoded(result.len().saturating_add(size), r.position())?;
        result.extend_from_slice(r.take(size)?);
    }
    Ok((Cow::Owned(result), binary))
}

/// The eexec section's plaintext: hexadecimal data decoded unless PFB
/// framing marked it binary, then decrypted if it was encrypted.
fn eexec<'a>(
    flat: &'a [u8],
    clear: &Clear,
    binary: bool,
    work: &mut Work<'_>,
) -> Result<Cow<'a, [u8]>> {
    let start = clear.start;
    let cipher = flat
        .get(start..)
        .ok_or_else(|| syntax(Position::Buffer(start), "missing eexec data"))?;
    let decoded = if binary {
        None
    } else {
        cipher::hex(cipher, start, work)?
    };
    let cipher = decoded.map_or(Cow::Borrowed(cipher), Cow::Owned);
    if !clear.encrypted {
        return Ok(cipher);
    }
    Ok(Cow::Owned(decrypt(
        &cipher,
        EEXEC_KEY,
        EEXEC_DISCARD,
        Position::Buffer(start),
    )?))
}

struct Clear {
    matrix: Transform,
    encoding: Box<[Option<Name>; 256]>,
    start: usize,
    encrypted: bool,
    aliases: Aliases,
}

fn cleartext(flat: &[u8], work: &mut Work<'_>) -> Result<Clear> {
    if !flat.starts_with(b"%!") {
        return Err(syntax(Position::Buffer(0), "missing Type 1 header"));
    }
    let mut clear = Tokens::new(flat, work);
    let mut aliases = Aliases::default();
    let mut matrix = None;
    let mut font_type = false;
    let mut encrypted = false;
    let mut encoding = None;
    let mut custom_encoding = false;
    let mut start = None;
    while let Some(token) = clear.token()? {
        match token {
            b"/FontMatrix" => matrix = Some(clear.matrix()?),
            b"/FontType" => {
                if clear.integer()? != FONT_TYPE {
                    return Err(syntax(clear.position(), "invalid Type 1 FontType"));
                }
                font_type = true;
            }
            b"/Encoding" => {
                let next = clear.required()?;
                if next == b"StandardEncoding" {
                    encoding = Some(strings::standard_encoding());
                } else if next == b"256" && clear.required()? == b"array" {
                    custom_encoding = true;
                    // Entries are added to the current encoding, if any.
                    encoding.get_or_insert_with(|| Box::new(std::array::from_fn(|_| None)));
                } else {
                    return Err(computed(
                        &clear,
                        "Type 1 encoding expression is not implemented",
                    ));
                }
            }
            b"dup" if custom_encoding => {
                // Initialisation procedures are skipped as balanced procedures;
                // explicit code/name entries outside them determine the encoding.
                let code = clear.integer()?;
                let name = clear.required()?;
                let name = name
                    .strip_prefix(b"/")
                    .ok_or_else(|| syntax(clear.position(), "invalid Type 1 encoding name"))?;
                let entry = encoding
                    .as_mut()
                    .and_then(|encoding| encoding.get_mut(code))
                    .ok_or_else(|| syntax(clear.position(), "Type 1 encoding code exceeds 255"))?;
                *entry = Some(Name::try_new(name).map_err(|e| e.at(clear.position()))?);
                if clear.required()? != b"put" {
                    return Err(computed(
                        &clear,
                        "Type 1 encoding expression is not implemented",
                    ));
                }
            }
            b"def" | b"readonly" if custom_encoding => custom_encoding = false,
            b"/Private" => {
                // An unencrypted font continues with its Private dictionary.
                start = Some(clear.token_start());
                break;
            }
            b"eexec" => {
                encrypted = true;
                break;
            }
            _ => aliases.definition(token, &mut clear)?,
        }
    }
    if !font_type {
        return Err(syntax(clear.position(), "Type 1 font has no FontType"));
    }
    let Some(encoding) = encoding else {
        return Err(syntax(clear.position(), "Type 1 font has no encoding"));
    };
    let matrix = matrix.ok_or_else(|| syntax(clear.position(), "missing Type 1 FontMatrix"))?;
    let mut start = start.unwrap_or(clear.offset());
    // One end-of-line ends the eexec operator (Type 1 7.2).
    if flat.get(start..start.saturating_add(2)) == Some(b"\r\n") {
        start += 2;
    } else if flat.get(start).is_some_and(u8::is_ascii_whitespace) {
        start += 1;
    }
    Ok(Clear {
        matrix,
        encoding,
        start,
        encrypted,
        aliases,
    })
}

fn computed(r: &Tokens<'_, '_, '_>, message: &'static str) -> pagura_core::Error {
    unsupported(features::TYPE1_POSTSCRIPT, r.position(), message)
}

/// The Private dictionary's subroutines and the CharStrings dictionary.
struct Private {
    subrs: Vec<Charstring>,
    charstrings: CharStrings,
}

fn private(plain: &[u8], mut aliases: Aliases, work: &mut Work<'_>) -> Result<Private> {
    let mut r = Tokens::new(plain, work);
    let mut len_iv = Some(DEFAULT_LEN_IV);
    let mut subrs = None;
    let mut charstrings = None;
    let mut closed = false;
    while let Some(token) = r.token()? {
        match token {
            b"hires" | b"FontDirectory" => {
                return Err(computed(
                    &r,
                    "hybrid or synthetic Type 1 font is not supported",
                ));
            }
            b"/lenIV" => {
                let token = r.required()?;
                len_iv = if token == b"-1" {
                    None
                } else {
                    Some(tokens::integer(token, r.position())?)
                };
            }
            b"/Subrs" => {
                if subrs.is_some() {
                    return Err(syntax(r.position(), "duplicate Type 1 Subrs array"));
                }
                subrs = Some(subroutines(&mut r, len_iv, &aliases)?);
            }
            b"/CharStrings" => {
                if charstrings.is_some() {
                    return Err(syntax(
                        r.position(),
                        "invalid Type 1 dictionary organization",
                    ));
                }
                let mut read = CharStrings::default();
                read.read(&mut r, len_iv, &aliases)?;
                charstrings = Some(read);
            }
            b"closefile" => {
                closed = true;
                break;
            }
            _ => aliases.definition(token, &mut r)?,
        }
    }
    if !closed {
        return Err(syntax(r.position(), "missing Type 1 closefile"));
    }
    let mut charstrings = charstrings.unwrap_or_default();
    // The plaintext start: CharStrings has no glyph to point at.
    charstrings.normalize(Position::Buffer(0))?;
    Ok(Private {
        subrs: subrs.unwrap_or_default(),
        charstrings,
    })
}

fn subroutines(
    r: &mut Tokens<'_, '_, '_>,
    len_iv: Option<usize>,
    aliases: &Aliases,
) -> Result<Vec<Charstring>> {
    let count = r.integer()?;
    let at = r.position();
    r.work().entries(count, at)?;
    let mut slots: Vec<Option<Charstring>> = std::iter::repeat_with(|| None).take(count).collect();
    if r.required()? != b"array" {
        return Err(syntax(r.position(), "invalid Type 1 Subrs array"));
    }
    while r.peek()? == Some(b"dup".as_slice()) {
        r.required()?;
        let index = r.integer()?;
        let (program, start) = r.binary(aliases)?;
        let program = charstring_bytes(program, start, len_iv)?;
        let slot = slots
            .get_mut(index)
            .ok_or_else(|| syntax(r.position(), "Type 1 Subrs index out of range"))?;
        if slot.is_some() {
            return Err(syntax(r.position(), "duplicate Type 1 subroutine"));
        }
        *slot = Some(program);
        aliases.finish(r, Assignment::Put)?;
    }
    let mut subrs = Vec::with_capacity(count);
    for slot in slots {
        subrs.push(slot.ok_or_else(|| syntax(r.position(), "incomplete Type 1 Subrs array"))?);
    }
    Ok(subrs)
}
