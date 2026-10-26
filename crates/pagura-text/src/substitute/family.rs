//! Bounded reader policy for bundled face selection (9.6.2.2, 9.8.1–9.8.2).
use crate::descriptor::FontFlags;

macro_rules! standard14 {
    ($($variant:ident => $name:literal),* $(,)?) => {
        /// The standard 14 fonts (9.6.2.2), by PostScript name.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) enum Standard14 {
            $($variant),*
        }

        impl Standard14 {
            /// The canonical PostScript name a provider looks up.
            pub(crate) const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name),*
                }
            }

            /// The informational message of a substitution by this face.
            pub(crate) const fn substitution_message(self) -> &'static str {
                match self {
                    $(Self::$variant => concat!("substitute outlines for ", $name)),*
                }
            }
        }
    };
}

standard14! {
    Courier => "Courier",
    CourierBold => "Courier-Bold",
    CourierOblique => "Courier-Oblique",
    CourierBoldOblique => "Courier-BoldOblique",
    TimesRoman => "Times-Roman",
    TimesBold => "Times-Bold",
    TimesItalic => "Times-Italic",
    TimesBoldItalic => "Times-BoldItalic",
    Helvetica => "Helvetica",
    HelveticaBold => "Helvetica-Bold",
    HelveticaOblique => "Helvetica-Oblique",
    HelveticaBoldOblique => "Helvetica-BoldOblique",
    Symbol => "Symbol",
    ZapfDingbats => "ZapfDingbats",
}

impl Standard14 {
    /// Symbol and ZapfDingbats use their own built-in encodings (9.6.5.1).
    pub(crate) const fn is_symbolic(self) -> bool {
        matches!(self, Self::Symbol | Self::ZapfDingbats)
    }
}

/// The Latin family a face is substituted from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Family {
    Courier,
    Times,
    Helvetica,
}

/// Weight and slant of a face.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Style {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl Family {
    /// FixedPitch selects Courier, Serif selects Times, otherwise Helvetica.
    pub(super) const fn from_flags(flags: FontFlags) -> Self {
        if flags.contains(FontFlags::FIXED_PITCH) {
            Self::Courier
        } else if flags.contains(FontFlags::SERIF) {
            Self::Times
        } else {
            Self::Helvetica
        }
    }

    pub(super) const fn face(self, style: Style) -> Standard14 {
        match (self, style) {
            (Self::Courier, Style::Regular) => Standard14::Courier,
            (Self::Courier, Style::Bold) => Standard14::CourierBold,
            (Self::Courier, Style::Italic) => Standard14::CourierOblique,
            (Self::Courier, Style::BoldItalic) => Standard14::CourierBoldOblique,
            (Self::Times, Style::Regular) => Standard14::TimesRoman,
            (Self::Times, Style::Bold) => Standard14::TimesBold,
            (Self::Times, Style::Italic) => Standard14::TimesItalic,
            (Self::Times, Style::BoldItalic) => Standard14::TimesBoldItalic,
            (Self::Helvetica, Style::Regular) => Standard14::Helvetica,
            (Self::Helvetica, Style::Bold) => Standard14::HelveticaBold,
            (Self::Helvetica, Style::Italic) => Standard14::HelveticaOblique,
            (Self::Helvetica, Style::BoldItalic) => Standard14::HelveticaBoldOblique,
        }
    }
}

/// The longest base font name the alias policy inspects. Longer names are
/// not standard spellings, and the cap keeps the comparison buffer small.
const MAX_NAME: usize = 80;

/// Recognizes known family/style spellings. A subset tag is exactly six
/// uppercase ASCII letters followed by `+` (9.9.2); aliases are reader policy.
pub(crate) fn standard_name(name: &[u8]) -> Option<Standard14> {
    let mut buffer = [0; MAX_NAME];
    let normalized = normalized(name, &mut buffer)?;
    let normalized = normalized.strip_suffix(b"mt").unwrap_or(normalized);
    for (prefix, family) in [
        (b"timesnewromanps".as_slice(), Family::Times),
        (b"timesnewroman", Family::Times),
        (b"couriernewps", Family::Courier),
        (b"couriernew", Family::Courier),
        (b"helvetica", Family::Helvetica),
        (b"arial", Family::Helvetica),
        (b"courier", Family::Courier),
        (b"times", Family::Times),
    ] {
        if let Some(style) = normalized.strip_prefix(prefix).and_then(style) {
            return Some(family.face(style));
        }
    }
    // The bundled symbol programs have only regular outlines. Known style
    // suffixes keep the family's encoding and metrics without synthesizing
    // a different face; the substitution diagnostic names that regular face.
    for (prefix, face) in [
        (b"symbol".as_slice(), Standard14::Symbol),
        (b"zapfdingbats", Standard14::ZapfDingbats),
        (b"dingbats", Standard14::ZapfDingbats),
    ] {
        if normalized.strip_prefix(prefix).and_then(style).is_some() {
            return Some(face);
        }
    }
    None
}

fn style(suffix: &[u8]) -> Option<Style> {
    Some(match suffix {
        b"" | b"roman" | b"regular" => Style::Regular,
        b"bold" | b"b" | b"bb" => Style::Bold,
        b"italic" | b"oblique" | b"i" => Style::Italic,
        b"bolditalic" | b"boldoblique" | b"bi" | b"ib" => Style::BoldItalic,
        _ => return None,
    })
}

/// Lowercases `name` without its subset tag, hyphens, commas and spaces.
fn normalized<'a>(name: &[u8], buffer: &'a mut [u8; MAX_NAME]) -> Option<&'a [u8]> {
    let name = crate::encoding::without_subset_tag(name);
    if name.len() > MAX_NAME {
        return None;
    }
    let mut len = 0;
    for byte in name.iter().filter(|b| !matches!(b, b'-' | b',' | b' ')) {
        buffer[len] = byte.to_ascii_lowercase();
        len += 1;
    }
    Some(&buffer[..len])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_and_messages() {
        assert_eq!(
            standard_name(b"Arial,BoldItalic"),
            Some(Standard14::HelveticaBoldOblique)
        );
        assert_eq!(
            standard_name(b"ABCDEF+TimesNewRomanPSMT"),
            Some(Standard14::TimesRoman)
        );
        assert_eq!(standard_name(b"Symbol,Italic"), Some(Standard14::Symbol));
        assert_eq!(standard_name(b"Unknown"), None);
        assert_eq!(standard_name(&[b'A'; MAX_NAME + 1]), None);
        assert_eq!(
            Standard14::CourierBold.substitution_message(),
            "substitute outlines for Courier-Bold"
        );
        assert_eq!(
            Family::from_flags(FontFlags::new(2)).face(Style::BoldItalic),
            Standard14::TimesBoldItalic
        );
    }
}
