//! Bounded glyph-index lookup of names, independent of PDF encodings.
use crate::{GlyphId, memory, read};
use pagura_core::{Limits, Name, Result};

pub(crate) fn indexed<'a>(
    count: usize,
    entries: impl Iterator<Item = Result<(GlyphId, &'a Name)>>,
    limits: &Limits,
    at: usize,
) -> Result<Vec<Option<Name>>> {
    let mut entries = entries.peekable();
    if entries.peek().is_none() {
        return Ok(Vec::new());
    }
    let mut bytes = count
        .checked_mul(size_of::<Option<Name>>())
        .filter(|n| isize::try_from(*n).is_ok())
        .ok_or_else(|| read::limit(at, "glyph-name index size overflow"))?;
    read::cap(bytes, limits.decoded_bytes, at)?;
    let mut names = vec![None; count];
    for entry in entries {
        let (glyph, name) = entry?;
        bytes = bytes
            .checked_add(name.len())
            .ok_or_else(|| read::limit(at, "glyph-name byte count overflow"))?;
        read::cap(bytes, limits.decoded_bytes, at)?;
        let slot = usize::try_from(glyph.0)
            .ok()
            .and_then(|i| names.get_mut(i))
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
    use super::*;
    use pagura_core::ErrorKind;

    #[test]
    fn repeated_name_payloads_are_bounded_before_cloning() {
        let name = Name::new(&[b'x'; 200]);
        let limits = Limits {
            decoded_bytes: 400,
            ..Limits::default()
        };
        let error = indexed(4, (0..4).map(|i| Ok((GlyphId(i), &name))), &limits, 123).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Limit);
        assert_eq!(error.position, pagura_core::Position::Buffer(123));
        assert!(
            indexed(
                usize::MAX,
                std::iter::once(Ok((GlyphId(0), &name))),
                &limits,
                123
            )
            .is_err()
        );
        assert_eq!(
            indexed(usize::MAX, std::iter::empty(), &limits, 123).unwrap(),
            Vec::<Option<Name>>::new()
        );
    }

    #[test]
    fn invalid_index_retains_the_table_position() {
        let name = Name::new(b"A");
        let error = indexed(
            1,
            std::iter::once(Ok((GlyphId(1), &name))),
            &Limits::default(),
            456,
        )
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.position, pagura_core::Position::Buffer(456));
    }
}
