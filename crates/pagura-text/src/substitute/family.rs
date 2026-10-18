//! Bounded reader policy for bundled face selection (9.8.1–9.8.2).

#[derive(Clone, Copy)]
pub(super) enum Family {
    Fixed,
    Serif,
    Sans,
}

impl Family {
    pub(super) fn from_flags(flags: u32) -> Self {
        if flags & 1 != 0 {
            return Self::Fixed;
        }
        if flags & 2 != 0 {
            return Self::Serif;
        }
        Self::Sans
    }

    pub(super) fn canonical(self, bold: bool, italic: bool) -> &'static [u8] {
        match (self, bold, italic) {
            (Self::Fixed, false, false) => b"Courier",
            (Self::Fixed, true, false) => b"Courier-Bold",
            (Self::Fixed, false, true) => b"Courier-Oblique",
            (Self::Fixed, true, true) => b"Courier-BoldOblique",
            (Self::Serif, false, false) => b"Times-Roman",
            (Self::Serif, true, false) => b"Times-Bold",
            (Self::Serif, false, true) => b"Times-Italic",
            (Self::Serif, true, true) => b"Times-BoldItalic",
            (Self::Sans, false, false) => b"Helvetica",
            (Self::Sans, true, false) => b"Helvetica-Bold",
            (Self::Sans, false, true) => b"Helvetica-Oblique",
            (Self::Sans, true, true) => b"Helvetica-BoldOblique",
        }
    }
}

/// Recognizes known family/style spellings. A subset tag is exactly six
/// uppercase ASCII letters followed by `+` (9.9.2); aliases are reader policy.
pub(crate) fn standard_name(name: &[u8]) -> Option<&'static [u8]> {
    let normalized = normalized(name)?;
    let normalized = normalized.strip_suffix(b"mt").unwrap_or(&normalized);
    for (prefix, family) in [
        (b"timesnewromanps".as_slice(), Family::Serif),
        (b"timesnewroman", Family::Serif),
        (b"couriernewps", Family::Fixed),
        (b"couriernew", Family::Fixed),
        (b"helvetica", Family::Sans),
        (b"arial", Family::Sans),
        (b"courier", Family::Fixed),
        (b"times", Family::Serif),
    ] {
        if let Some(style) = normalized.strip_prefix(prefix)
            && let Some((bold, italic)) = style_flags(style)
        {
            return Some(family.canonical(bold, italic));
        }
    }
    // The bundled symbol programs have only regular outlines. Known style
    // suffixes keep the family's encoding and metrics without synthesizing
    // a different face; the substitution diagnostic names that regular face.
    for (prefix, canonical) in [
        (b"symbol".as_slice(), b"Symbol".as_slice()),
        (b"zapfdingbats", b"ZapfDingbats"),
        (b"dingbats", b"ZapfDingbats"),
    ] {
        if normalized
            .strip_prefix(prefix)
            .and_then(style_flags)
            .is_some()
        {
            return Some(canonical);
        }
    }
    None
}

fn style_flags(style: &[u8]) -> Option<(bool, bool)> {
    Some(match style {
        b"" | b"roman" | b"regular" => (false, false),
        b"bold" | b"b" | b"bb" => (true, false),
        b"italic" | b"oblique" | b"i" => (false, true),
        b"bolditalic" | b"boldoblique" | b"bi" | b"ib" => (true, true),
        _ => return None,
    })
}

fn normalized(name: &[u8]) -> Option<Vec<u8>> {
    let name = if name.get(6) == Some(&b'+') && name.get(..6)?.iter().all(u8::is_ascii_uppercase) {
        name.get(7..)?
    } else {
        name
    };
    // Never allocate in proportion to an arbitrarily large PDF name/string.
    (name.len() <= 80).then(|| {
        name.iter()
            .filter(|b| !matches!(b, b'-' | b',' | b' '))
            .map(u8::to_ascii_lowercase)
            .collect()
    })
}
