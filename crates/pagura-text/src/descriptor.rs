//! Font descriptors (9.8): flags and embedded font programs.
use crate::{objects::optional, repairs, report::Report};
use pagura_core::{Diagnostic, Dictionary, ObjectId, Position, Result, Value};
use pagura_document::Document;

/// The keys of an embedded font program, in lookup order (Table 124).
const PROGRAM_KEYS: [&[u8]; 3] = [b"FontFile", b"FontFile2", b"FontFile3"];

/// The `/Flags` bit field of a font descriptor (9.8.2, Table 121).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FontFlags(u32);

impl FontFlags {
    /// All glyphs have the same width.
    pub(crate) const FIXED_PITCH: Self = Self(1 << 0);
    /// Glyphs have serifs.
    pub(crate) const SERIF: Self = Self(1 << 1);
    /// The font uses characters outside the Standard Latin character set.
    pub(crate) const SYMBOLIC: Self = Self(1 << 2);
    /// The font uses only the Standard Latin character set.
    pub(crate) const NONSYMBOLIC: Self = Self(1 << 5);
    /// Glyphs have dominant vertical strokes that are slanted.
    pub(crate) const ITALIC: Self = Self(1 << 6);
    /// Bold glyphs are painted with extra pixels at small sizes.
    pub(crate) const FORCE_BOLD: Self = Self(1 << 18);

    pub(crate) const fn new(bits: u32) -> Self {
        Self(bits)
    }

    pub(crate) const fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 == flag.0
    }

    /// Whether exactly one of Symbolic and Nonsymbolic is set, as 9.8.2
    /// requires.
    pub(crate) const fn is_consistent(self) -> bool {
        self.contains(Self::SYMBOLIC) != self.contains(Self::NONSYMBOLIC)
    }

    /// These flags with Nonsymbolic set exactly when Symbolic is clear:
    /// Symbolic is the primary bit (9.8.2).
    const fn with_primary_symbolic(self) -> Self {
        if self.contains(Self::SYMBOLIC) {
            Self(self.0 & !Self::NONSYMBOLIC.0)
        } else {
            Self(self.0 | Self::NONSYMBOLIC.0)
        }
    }
}

/// A resolved font descriptor and its object, when it is indirect.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Descriptor<'a> {
    pub(crate) dictionary: &'a Dictionary,
    pub(crate) id: Option<ObjectId>,
}

impl<'a> Descriptor<'a> {
    /// The `/FontDescriptor` of a simple font or CIDFont. A null value or a
    /// reference to a missing object is absent (7.3.9, 7.3.10).
    pub(crate) fn of(document: &'a Document, font: &'a Dictionary) -> Result<Option<Self>> {
        let Some(value) = optional(document, font, b"FontDescriptor")? else {
            return Ok(None);
        };
        Ok(Some(Self {
            dictionary: document.resolve_dictionary(value)?,
            id: font.get(b"FontDescriptor").and_then(Value::as_reference),
        }))
    }

    /// `/Flags`, when it is an integer that fits the 32-bit field.
    pub(crate) fn flags(&self, document: &Document) -> Result<Option<FontFlags>> {
        Ok(optional(document, self.dictionary, b"Flags")?
            .and_then(Value::as_integer)
            .and_then(|value| u32::try_from(value).ok())
            .map(FontFlags::new))
    }

    /// `/Flags` with exactly one of Symbolic and Nonsymbolic. A missing,
    /// non-integer or contradictory value is the reported repair
    /// `font.descriptor-flags`, which keeps Symbolic as the primary bit and
    /// every other bit as written (9.8.2, Table 121). `position` locates the
    /// repair, for example at the operator that selected the font.
    pub(crate) fn normalized_flags(
        &self,
        document: &Document,
        position: Position,
    ) -> Result<FontFlags> {
        let flags = self.flags(document)?;
        if let Some(flags) = flags.filter(|flags| flags.is_consistent()) {
            return Ok(flags);
        }
        Report::new(document).diagnostic(
            Diagnostic::new(
                repairs::DESCRIPTOR_FLAGS,
                "font descriptor must set exactly one of Symbolic and Nonsymbolic (9.8.2, Table 121)",
            )
            .at(position)
            .in_object_opt(self.id),
        )?;
        Ok(flags.unwrap_or(FontFlags::new(0)).with_primary_symbolic())
    }

    /// The key of the embedded font program, if any. A key whose value is
    /// null or refers to a missing object embeds nothing, so a provider
    /// substitute can still apply.
    pub(crate) fn program_key(&self, document: &Document) -> Result<Option<&'static [u8]>> {
        for key in PROGRAM_KEYS {
            if optional(document, self.dictionary, key)?.is_some() {
                return Ok(Some(key));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_pairs() {
        assert!(FontFlags::new(4).is_consistent());
        assert!(FontFlags::new(32 | 64).is_consistent());
        assert!(!FontFlags::new(36).is_consistent());
        assert!(!FontFlags::new(0).is_consistent());
        assert!(FontFlags::new(1 << 18).contains(FontFlags::FORCE_BOLD));
        assert_eq!(
            FontFlags::new(36).with_primary_symbolic(),
            FontFlags::new(4)
        );
        assert_eq!(
            FontFlags::new(1).with_primary_symbolic(),
            FontFlags::new(33)
        );
    }

    #[test]
    fn normalization_keeps_symbolic_primary_on_the_generated_vectors() -> Result<()> {
        use pagura_document::OpenOptions;
        use pagura_testkit::{Manifest, fixture};
        for case in Manifest::load("font").cases() {
            let name = case.name();
            if !name.starts_with("descriptor-") {
                continue;
            }
            let document = Document::open(
                fixture("font", &format!("{name}.pdf")),
                &OpenOptions::default(),
            )?;
            let font = document
                .object(ObjectId::new(5, 0))?
                .dictionary()
                .ok_or(pagura_core::Error::syntax("missing font"))?;
            let descriptor = Descriptor::of(&document, font)?
                .ok_or(pagura_core::Error::syntax("missing descriptor"))?;
            let written = descriptor.flags(&document)?;
            let flags = descriptor.normalized_flags(&document, Position::Unknown)?;
            let expect =
                String::from_utf8_lossy(&fixture("font", &format!("{name}.expect"))).into_owned();
            let symbolic = expect.lines().any(|line| line == "symbolic 1");
            assert_eq!(flags.contains(FontFlags::SYMBOLIC), symbolic, "{name}");
            assert!(flags.is_consistent(), "{name}");
            let other = !(FontFlags::SYMBOLIC.0 | FontFlags::NONSYMBOLIC.0);
            assert_eq!(
                flags.0 & other,
                written.map_or(0, |written| written.0) & other,
                "{name}"
            );
            let repaired = fixture("font", &format!("{name}.strict")) != b"valid\n";
            assert_eq!(!document.diagnostics().is_empty(), repaired, "{name}");
        }
        Ok(())
    }
}
