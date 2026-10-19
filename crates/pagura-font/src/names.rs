//! Bounded glyph-index lookup of program glyph names, independent of PDF
//! encodings.
use crate::budget::Work;
use crate::{GlyphId, memory, read};
use pagura_core::convert::u32_to_usize;
use pagura_core::{Name, Position, Result};

/// The name of each of `count` glyphs, from `(glyph, name)` entries; an
/// empty index if there are none. Each slot is a held entry, and copying a
/// name is work in proportion to its length, so many glyphs sharing one
/// long name are bounded before the name is cloned for each.
pub(crate) fn indexed<'a>(
    count: usize,
    entries: impl Iterator<Item = Result<(GlyphId, &'a Name)>>,
    work: &mut Work<'_>,
    at: usize,
) -> Result<Vec<Option<Name>>> {
    let mut entries = entries.peekable();
    if entries.peek().is_none() {
        return Ok(Vec::new());
    }
    work.entries(count, Position::Buffer(at))?;
    let mut names = Vec::new();
    names
        .try_reserve_exact(count)
        .map_err(|_| read::limit(at, "glyph-name index allocation failed"))?;
    names.resize(count, None);
    for entry in entries {
        let (glyph, name) = entry?;
        work.work(name.len(), Position::Buffer(at))?;
        let slot = names
            .get_mut(u32_to_usize(glyph.0))
            .ok_or_else(|| read::bad(at, "glyph-name index outside program"))?;
        *slot = Some(name.clone());
    }
    Ok(names)
}

pub(crate) fn heap_bytes(names: &Vec<Option<Name>>) -> usize {
    names
        .iter()
        .flatten()
        .fold(memory::vector(names), |n, name| {
            n.saturating_add(name.len())
        })
}

#[cfg(test)]
mod tests {
    use super::indexed;
    use crate::GlyphId;
    use crate::budget::{FontBudget, Work};
    use pagura_core::{Diagnostics, ErrorKind, Limits, Name, Position, Recovery};

    #[test]
    fn repeated_name_payloads_are_bounded_before_cloning() {
        let name = Name::new(&[b'x'; 200]);
        let limits = Limits {
            font_work: 400,
            ..Limits::default()
        };
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(&limits), &mut diagnostics);
        let error =
            indexed(4, (0..4).map(|i| Ok((GlyphId(i), &name))), &mut work, 123).unwrap_err();
        assert_eq!(
            (error.kind, error.position),
            (ErrorKind::Limit, Position::Buffer(123))
        );
        let mut work = Work::new(FontBudget::new(&Limits::default()), &mut diagnostics);
        assert_eq!(
            indexed(
                usize::MAX,
                std::iter::once(Ok((GlyphId(0), &name))),
                &mut work,
                123
            )
            .unwrap_err()
            .kind,
            ErrorKind::Limit
        );
        assert_eq!(
            indexed(usize::MAX, std::iter::empty(), &mut work, 123).unwrap(),
            []
        );
    }

    #[test]
    fn an_index_outside_the_program_is_malformed() {
        let name = Name::new(b"A");
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let mut work = Work::new(FontBudget::new(&Limits::default()), &mut diagnostics);
        let error =
            indexed(1, std::iter::once(Ok((GlyphId(1), &name))), &mut work, 456).unwrap_err();
        assert_eq!(
            (error.kind, error.position),
            (ErrorKind::Syntax, Position::Buffer(456))
        );
    }
}
