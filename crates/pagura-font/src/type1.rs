//! Static Type 1 font dictionaries, PFA/PFB framing and cipher decoding.
//!
//! Follows Adobe Type 1 Font Format, referenced by ISO 32000-2, 9.9.
//! This is a font reader, without a PostScript execution environment.
use crate::GlyphId;
use crate::memory::{encoding, names, nested};
use crate::read::{bad, cap};
use pagura_core::{Diagnostics, Limits, Name, Position, Recovery, Result};
use pagura_raster::{Path, Transform};
use std::collections::BTreeMap;

mod aliases;
mod charstring;
mod charstrings;
use charstrings::CharStrings;
mod tokens;
use tokens::Tokens;

#[derive(Debug)]
pub(crate) struct Type1 {
    glyphs: Vec<Vec<u8>>,
    subrs: Vec<Vec<u8>>,
    names: BTreeMap<Name, GlyphId>,
    glyph_names: Vec<Option<Name>>,
    encoding: Box<[Option<Name>; 256]>,
    matrix: Transform,
}

pub(super) fn decrypt(data: &[u8], seed: u16, discard: usize, at: usize) -> Result<Vec<u8>> {
    if data.len() < discard {
        return Err(bad(at, "truncated Type 1 cipher prefix"));
    }
    let mut state = seed;
    let mut result = Vec::with_capacity(data.len() - discard);
    for (i, &byte) in data.iter().enumerate() {
        let plain = byte ^ state.to_be_bytes()[0];
        state = u16::from(byte)
            .wrapping_add(state)
            .wrapping_mul(52845)
            .wrapping_add(22719);
        if i >= discard {
            result.push(plain);
        }
    }
    Ok(result)
}

impl Type1 {
    pub(crate) fn parse(
        data: &[u8],
        limits: &Limits,
        diagnostics: &mut Diagnostics,
    ) -> Result<Self> {
        let (flat, binary) = flatten(data, limits)?;
        let Clear {
            matrix,
            encoding,
            start,
            encrypted,
            mut aliases,
        } = cleartext(&flat, limits)?;
        let cipher = flat
            .get(start..)
            .ok_or_else(|| bad(start, "missing eexec data"))?;
        let cipher = if binary {
            cipher.to_vec()
        } else {
            eexec_bytes(cipher, limits)?
        };
        let plain = if encrypted {
            decrypt(&cipher, 55665, 4, start)?
        } else {
            cipher
        };
        let mut r = Tokens::new(&plain, limits);
        let mut len_iv = Some(4);
        let mut subrs = Vec::new();
        let mut have_subrs = false;
        let mut chars = CharStrings::default();
        let mut closed = false;
        let mut have_chars = false;
        while let Some(token) = r.next()? {
            match token {
                b"hires" | b"FontDirectory" => {
                    return Err(unsupported(
                        r.pos,
                        "hybrid or synthetic Type 1 font is not supported",
                    ));
                }
                b"/lenIV" => {
                    let token = r.required()?;
                    if token == b"-1" {
                        len_iv = None;
                    } else {
                        len_iv = Some(tokens::integer(token, r.pos)?);
                    }
                }
                b"/Subrs" => {
                    if have_subrs {
                        return Err(bad(r.pos, "duplicate Type 1 Subrs array"));
                    }
                    have_subrs = true;
                    subrs = subroutines(&mut r, len_iv, limits, &aliases)?;
                }
                b"/CharStrings" => {
                    if have_chars {
                        return Err(bad(r.pos, "invalid Type 1 dictionary organization"));
                    }
                    have_chars = true;
                    chars.read(&mut r, len_iv, limits, subrs.len(), &aliases, diagnostics)?;
                }
                b"closefile" => {
                    closed = true;
                    break;
                }
                _ => aliases.definition(token, &mut r, limits)?,
            }
        }
        if !closed {
            return Err(bad(r.pos, "missing Type 1 closefile"));
        }
        chars.normalize(start)?;
        let CharStrings { glyphs, names } = chars;
        let glyph_names = crate::names::indexed(
            glyphs.len(),
            names.iter().map(|(name, glyph)| Ok((*glyph, name))),
            limits,
            start,
        )?;
        Ok(Self {
            glyphs,
            subrs,
            names,
            glyph_names,
            encoding,
            matrix,
        })
    }
    pub(crate) fn glyph_count(&self) -> u32 {
        u32::try_from(self.glyphs.len()).unwrap_or(0)
    }
    pub(crate) fn matrix(&self) -> Transform {
        self.matrix
    }
    pub(crate) fn glyph_name(&self, glyph: GlyphId) -> Option<&Name> {
        self.glyph_names
            .get(usize::try_from(glyph.0).ok()?)?
            .as_ref()
    }
    pub(crate) fn name(&self, name: &[u8]) -> Option<GlyphId> {
        self.names.get(name).copied()
    }
    pub(crate) fn encoding(&self) -> &[Option<Name>; 256] {
        &self.encoding
    }
    pub(crate) fn outline(
        &self,
        glyph: GlyphId,
        limits: &Limits,
        diagnostics: &mut Diagnostics,
    ) -> Result<Path> {
        Ok(charstring::glyph(self, glyph, limits, diagnostics)?.0)
    }
    pub(crate) fn advance(&self, glyph: GlyphId, limits: &Limits) -> Option<f64> {
        charstring::glyph(self, glyph, limits, &mut Diagnostics::new(Recovery::Strict))
            .ok()
            .map(|g| g.1)
    }

    /// Type 1 keeps decrypted charstrings and subroutines, not decrypted
    /// dictionaries.
    pub(crate) fn heap_bytes(&self) -> usize {
        nested(&self.glyphs)
            .saturating_add(nested(&self.subrs))
            .saturating_add(names(&self.names))
            .saturating_add(crate::names::heap_bytes(&self.glyph_names))
            .saturating_add(encoding(&self.encoding))
    }
}

fn char_bytes(bytes: &[u8], len_iv: Option<usize>, at: usize) -> Result<Vec<u8>> {
    len_iv.map_or_else(|| Ok(bytes.to_vec()), |n| decrypt(bytes, 4330, n, at))
}
fn unsupported(at: usize, message: &'static str) -> pagura_core::Error {
    pagura_core::Error::unsupported(crate::features::TYPE1_POSTSCRIPT, message)
        .at(Position::Buffer(at))
}
fn flatten(data: &[u8], limits: &Limits) -> Result<(Vec<u8>, bool)> {
    if data.first() != Some(&128) {
        return Ok((data.to_vec(), false));
    }
    let mut pos = 0;
    let mut result = Vec::new();
    let mut segments = 0;
    let mut binary = false;
    loop {
        segments += 1;
        cap(segments, limits.operations, pos)?;
        if data.get(pos) != Some(&128) {
            return Err(bad(pos, "invalid PFB segment marker"));
        }
        let kind = *data
            .get(pos + 1)
            .ok_or_else(|| bad(pos, "truncated PFB header"))?;
        pos += 2;
        if kind == 3 {
            if pos != data.len() {
                return Err(bad(pos, "trailing PFB data"));
            }
            break;
        }
        if !matches!(kind, 1 | 2) {
            return Err(bad(pos, "invalid PFB segment kind"));
        }
        binary |= kind == 2;
        let bytes = data
            .get(pos..pos.saturating_add(4))
            .ok_or_else(|| bad(pos, "truncated PFB size"))?;
        let size = usize::try_from(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .map_err(|_| crate::read::limit(pos, "PFB size overflow"))?;
        pos += 4;
        let end = pos
            .checked_add(size)
            .ok_or_else(|| crate::read::limit(pos, "PFB size overflow"))?;
        cap(result.len().saturating_add(size), limits.decoded_bytes, pos)?;
        result.extend_from_slice(
            data.get(pos..end)
                .ok_or_else(|| bad(pos, "truncated PFB segment"))?,
        );
        pos = end;
    }
    Ok((result, binary))
}
fn eexec_bytes(data: &[u8], limits: &Limits) -> Result<Vec<u8>> {
    if !data
        .iter()
        .filter(|b| !b.is_ascii_whitespace())
        .take(4)
        .all(u8::is_ascii_hexdigit)
    {
        return Ok(data.to_vec());
    }
    let mut result = Vec::new();
    let mut high = None;
    for (i, &byte) in data.iter().enumerate() {
        if byte.is_ascii_whitespace() {
            continue;
        }
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'A'..=b'F' => byte - b'A' + 10,
            b'a'..=b'f' => byte - b'a' + 10,
            _ => break,
        };
        if let Some(h) = high.take() {
            cap(result.len() + 1, limits.decoded_bytes, i)?;
            result.push(h * 16 + digit);
        } else {
            high = Some(digit);
        }
    }
    if high.is_some() {
        return Err(bad(data.len(), "odd eexec hexadecimal data"));
    }
    Ok(result)
}

struct Clear {
    matrix: Transform,
    encoding: Box<[Option<Name>; 256]>,
    start: usize,
    encrypted: bool,
    aliases: aliases::Aliases,
}
fn cleartext(flat: &[u8], limits: &Limits) -> Result<Clear> {
    if !flat.starts_with(b"%!") {
        return Err(bad(0, "missing Type 1 header"));
    }
    let mut clear = Tokens::new(flat, limits);
    let mut aliases = aliases::Aliases::default();
    let mut matrix = None;
    let mut font_type = false;
    let mut encrypted = false;
    let mut encoding = Box::new(std::array::from_fn(|_| None));
    let mut encoded = false;
    let mut custom_encoding = false;
    while let Some(token) = clear.next()? {
        match token {
            b"/FontMatrix" => matrix = Some(clear.matrix()?),
            b"/FontType" => {
                if clear.integer()? != 1 {
                    return Err(bad(clear.pos, "invalid Type 1 FontType"));
                }
                font_type = true;
            }
            b"/Encoding" => {
                let next = clear.required()?;
                if next == b"StandardEncoding" {
                    for code in 0..=255_u8 {
                        encoding[usize::from(code)] = crate::cff::strings::standard(code);
                    }
                    encoded = true;
                } else if next == b"256" && clear.required()? == b"array" {
                    custom_encoding = true;
                    encoded = true;
                } else {
                    return Err(unsupported(
                        clear.pos,
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
                    .ok_or_else(|| bad(clear.pos, "invalid Type 1 encoding name"))?;
                let entry = encoding
                    .get_mut(code)
                    .ok_or_else(|| bad(clear.pos, "Type 1 encoding code exceeds 255"))?;
                *entry = Some(Name::new(name));
                if clear.required()? != b"put" {
                    return Err(unsupported(
                        clear.pos,
                        "Type 1 encoding expression is not implemented",
                    ));
                }
            }
            b"def" | b"readonly" if custom_encoding => custom_encoding = false,
            b"/Private" => {
                clear.pos -= token.len();
                break;
            }
            b"eexec" => {
                encrypted = true;
                break;
            }
            _ => aliases.definition(token, &mut clear, limits)?,
        }
    }
    if !font_type {
        return Err(bad(clear.pos, "Type 1 font has no FontType"));
    }
    if !encoded {
        return Err(bad(clear.pos, "Type 1 font has no encoding"));
    }
    let mut start = clear.pos;
    if flat.get(start) == Some(&b'\r') && flat.get(start + 1) == Some(&b'\n') {
        start += 2;
    } else if flat.get(start).is_some_and(u8::is_ascii_whitespace) {
        start += 1;
    }
    Ok(Clear {
        matrix: matrix.ok_or_else(|| bad(0, "missing Type 1 FontMatrix"))?,
        encoding,
        start,
        encrypted,
        aliases,
    })
}

fn subroutines(
    r: &mut Tokens<'_>,
    len_iv: Option<usize>,
    limits: &Limits,
    aliases: &aliases::Aliases,
) -> Result<Vec<Vec<u8>>> {
    let count = r.integer()?;
    cap(
        count,
        limits
            .objects
            .min(limits.value_nodes)
            .min(limits.decoded_bytes / size_of::<Vec<u8>>()),
        r.pos,
    )?;
    let mut subrs = vec![Vec::new(); count];
    let mut assigned = vec![false; count];
    let mut assignments = 0;
    if r.required()? != b"array" {
        return Err(bad(r.pos, "invalid Type 1 Subrs array"));
    }
    while r.peek()? == Some(b"dup".as_slice()) {
        r.required()?;
        let index = r.integer()?;
        let program = r.binary(aliases)?;
        let program = char_bytes(program, len_iv, r.pos)?;
        let slot = subrs
            .get_mut(index)
            .ok_or_else(|| bad(r.pos, "Type 1 Subrs index out of range"))?;
        if assigned[index] {
            return Err(bad(r.pos, "duplicate Type 1 subroutine"));
        }
        assigned[index] = true;
        assignments += 1;
        *slot = program;
        aliases.finish(r, aliases::Assignment::Put)?;
    }
    if assignments != count {
        return Err(bad(r.pos, "incomplete Type 1 Subrs array"));
    }
    Ok(subrs)
}
