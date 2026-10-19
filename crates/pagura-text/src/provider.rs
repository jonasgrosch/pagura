//! Host-supplied data absent from a PDF (ADR 0009).
use crate::{
    GlyphText, WritingMode,
    cid::Cid,
    cmap::{CMap, CMapRole},
};
use pagura_core::{Error, Limits, Meter, Name, Result, syntax::Origin};
use pagura_font::FontProgram;
use std::{collections::BTreeMap, sync::Arc};

/// External font resources. Implementations own their caches and budgets;
/// interpretation never opens files or acquires host capabilities.
/// Shared results are excluded from [`crate::PdfFont::retained_bytes`]; a
/// provider must budget them once in its own cache, independently of fonts.
/// The host must cover outstanding shared results even after cache eviction
/// or destruction of the provider itself.
pub trait Provider: Send + Sync {
    /// A horizontal substitute with a Unicode cmap for a CID collection.
    /// Selection translates collection CIDs through [`Self::cid_to_unicode`],
    /// never through the substitute's internal CIDs (9.7.4.2; ADR 0009).
    ///
    /// # Errors
    /// Returns parsing or resource-budget errors; `None` means unavailable.
    fn cid_font(&self, registry: &[u8], ordering: &[u8]) -> Result<Option<Arc<FontProgram>>>;
    /// Standard-14 metrics and optional substitute, by canonical base name.
    ///
    /// # Errors
    /// Returns parsing or resource-budget errors; `None` means unavailable.
    fn standard_font(&self, base_name: &[u8]) -> Result<Option<Arc<StandardFont>>>;
    /// A predefined character-code to CID CMap (9.7.5.2, Table 116).
    ///
    /// # Errors
    /// Returns parsing or resource-budget errors; `None` means unavailable.
    fn predefined_cmap(&self, name: &[u8]) -> Result<Option<PredefinedCMap>>;
    /// CID to Unicode data for a registry and ordering (9.10.2).
    ///
    /// # Errors
    /// Returns parsing or resource-budget errors; `None` means unavailable.
    fn cid_to_unicode(&self, registry: &[u8], ordering: &[u8])
    -> Result<Option<Arc<CidToUnicode>>>;
}

/// AFM font-wide descriptor values, in thousandths of text space. Start
/// from [`Default`] and set the values a metrics file records.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct StandardDescriptor {
    /// Font bounding box: left, bottom, right, top.
    pub bbox: [f64; 4],
    /// Italic angle in degrees.
    pub italic_angle: f64,
    /// Fixed-pitch property.
    pub fixed_pitch: bool,
    /// Ascender, when recorded.
    pub ascent: Option<f64>,
    /// Descender, when recorded.
    pub descent: Option<f64>,
    /// Capital height, when recorded.
    pub cap_height: Option<f64>,
    /// Lowercase x height, when recorded.
    pub x_height: Option<f64>,
    /// Dominant vertical stem width, when recorded.
    pub stem_v: Option<f64>,
    /// Dominant horizontal stem width, when recorded.
    pub stem_h: Option<f64>,
    /// AFM weight name.
    pub weight: Box<str>,
}

/// Standard-font metrics and its built-in code-to-name encoding (9.6.2.2).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct StandardMetrics {
    /// Canonical PostScript font name.
    pub name: Name,
    /// Built-in encoding, including undefined codes.
    pub encoding: [Option<Name>; 256],
    /// Widths by glyph name, in thousandths of text space.
    pub widths: BTreeMap<Name, f64>,
    /// Font-wide descriptor values.
    pub descriptor: StandardDescriptor,
}

impl StandardMetrics {
    /// Empty metrics for the font `name`: no encoded codes and no widths.
    pub fn new(name: Name) -> Self {
        Self {
            name,
            encoding: std::array::from_fn(|_| None),
            widths: BTreeMap::new(),
            descriptor: StandardDescriptor::default(),
        }
    }

    /// Width of a code in the built-in encoding, in AFM units.
    pub fn width(&self, code: u8) -> Option<f64> {
        self.encoding[usize::from(code)]
            .as_ref()
            .and_then(|name| self.widths.get(name))
            .copied()
    }
}

/// Shared metrics and a substitute outline program. Missing outlines do not
/// prevent metric lookup.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct StandardFont {
    /// Original metrics and encoding; these determine layout.
    pub metrics: Arc<StandardMetrics>,
    /// Replacement outlines, whose own advances do not override PDF/AFM widths.
    pub program: Option<Arc<FontProgram>>,
}

impl StandardFont {
    /// Metrics with optional replacement outlines.
    pub fn new(metrics: Arc<StandardMetrics>, program: Option<Arc<FontProgram>>) -> Self {
        Self { metrics, program }
    }
}

/// A parsed, immutable predefined CMap, including resolved `usecmap`
/// parents. Cloning shares the parsed map.
#[derive(Debug, Clone)]
pub struct PredefinedCMap(pub(crate) Arc<CMap>);

/// One code decoded by a predefined CMap (9.7.6.2–9.7.6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct CMapCode {
    /// The character code.
    pub code: u32,
    /// Bytes the code occupies.
    pub byte_len: u8,
    /// Its CID, or the notdef CID when the code maps to none.
    pub cid: u16,
}

impl PredefinedCMap {
    /// Parses the declarative CMap subset of TN5014 sections 5.1–5.4.
    /// `parent` resolves a `usecmap` name; its implementation must bound graph
    /// depth and reject cycles. Child mappings override inherited mappings,
    /// which are shared rather than copied.
    ///
    /// # Errors
    /// Returns syntax, unsupported-operator, parent-resolution or limit errors.
    pub fn parse(
        bytes: &[u8],
        limits: &Limits,
        mut parent: impl FnMut(&[u8]) -> Result<Self>,
    ) -> Result<Self> {
        if bytes.len() > limits.decoded_bytes {
            return Err(Error::limit("CMap resource byte budget exceeded"));
        }
        let mut map = CMap::parse(bytes, Origin::Buffer, limits, CMapRole::Encoding)?;
        if let Some(name) = map.use_name().cloned() {
            map.inherit_resource(parent(name.as_bytes())?.0)?;
        }
        map.validate(&mut Meter::new(
            limits.font_work,
            "CMap work budget exceeded",
        ))?;
        Ok(Self(Arc::new(map)))
    }

    /// Whether two handles share one parsed map.
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        Arc::ptr_eq(&a.0, &b.0)
    }

    /// Writing mode of this CMap.
    pub fn writing_mode(&self) -> WritingMode {
        self.0.mode
    }

    /// Registry and ordering from CIDSystemInfo, when present.
    pub fn collection(&self) -> Option<(&[u8], &[u8])> {
        self.0.collection()
    }

    /// Decodes the first code of `bytes`, with the CMap's notdef mapping for
    /// a code without a CID. Unlike [`crate::PdfFont::decode`], it does not
    /// check whether a font program has a glyph for the CID.
    ///
    /// # Errors
    /// Returns syntax errors for empty input and errors for exceeded budgets.
    pub fn decode_code(&self, bytes: &[u8], limits: &Limits) -> Result<CMapCode> {
        if bytes.is_empty() {
            return Err(Error::syntax("empty CMap input"));
        }
        let mut work = Meter::new(limits.font_work, "CMap decoding work budget exceeded");
        let (code, in_codespace) = self.0.next(bytes, &mut work)?;
        let mapped = if in_codespace {
            self.0.cid(code, &mut work)?
        } else {
            None
        };
        let cid = match mapped {
            Some(cid) => cid,
            None => self.0.notdef(code, &mut work)?,
        };
        Ok(CMapCode {
            code: code.value,
            byte_len: code.len,
            cid: cid.get(),
        })
    }
}

/// Shared CID-to-Unicode values for one character collection, indexed
/// directly by CID up to the largest one present.
#[derive(Debug, Clone)]
pub struct CidToUnicode(Box<[Option<GlyphText>]>);

impl CidToUnicode {
    /// Validates bounded, single-collection mapping data.
    ///
    /// # Errors
    /// Returns a limit error for excessive entries/output or syntax errors for
    /// CIDs outside the 16-bit PDF character-identifier range.
    pub fn new(values: BTreeMap<u32, String>, limits: &Limits) -> Result<Self> {
        if values.len() > limits.font_entries {
            return Err(Error::limit("CID Unicode entry budget exceeded"));
        }
        let mut bytes = Meter::new(limits.decoded_bytes, "CID Unicode byte budget exceeded");
        let last = values
            .last_key_value()
            .map_or(Ok(None), |(&cid, _)| Cid::try_from(cid).map(Some))?;
        let mut table = vec![None; last.map_or(0, |cid| usize::from(cid.get()) + 1)];
        for (cid, text) in values {
            bytes.charge(text.len())?;
            table[usize::from(Cid::try_from(cid)?.get())] = Some(GlyphText::from(text));
        }
        Ok(Self(table.into()))
    }

    /// One Unicode scalar per CID, the shape of most collection data,
    /// without a `String` per entry.
    ///
    /// # Errors
    /// Returns a limit error for more than `font_entries` entries or a syntax
    /// error for a repeated CID.
    pub fn from_scalars(
        values: impl IntoIterator<Item = (u16, char)>,
        limits: &Limits,
    ) -> Result<Self> {
        let mut entries = Meter::new(limits.font_entries, "CID Unicode entry budget exceeded");
        let mut table = Vec::new();
        for (cid, scalar) in values {
            entries.charge(1)?;
            let index = usize::from(cid);
            if table.len() <= index {
                table.resize(index + 1, None);
            }
            if table[index].replace(GlyphText::from(scalar)).is_some() {
                return Err(Error::syntax("duplicate CID Unicode entry"));
            }
        }
        Ok(Self(table.into()))
    }

    /// Unicode for a CID. Absence remains unknown rather than guessed.
    pub fn get(&self, cid: u32) -> Option<&str> {
        Cid::try_from(cid)
            .ok()
            .and_then(|cid| self.text(cid))
            .map(GlyphText::as_str)
    }

    pub(crate) fn text(&self, cid: Cid) -> Option<&GlyphText> {
        self.0.get(usize::from(cid.get()))?.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_match_strings_and_reject_repeats() -> Result<()> {
        let limits = Limits::default();
        let scalars = CidToUnicode::from_scalars([(3, 'A'), (1, '\u{1f600}')], &limits)?;
        let strings = CidToUnicode::new(
            BTreeMap::from([(3, "A".to_owned()), (1, "\u{1f600}".to_owned())]),
            &limits,
        )?;
        for cid in 0..5 {
            assert_eq!(scalars.get(cid), strings.get(cid), "{cid}");
        }
        assert!(CidToUnicode::from_scalars([(1, 'A'), (1, 'B')], &limits).is_err());
        let small = Limits {
            font_entries: 1,
            ..Limits::default()
        };
        assert!(CidToUnicode::from_scalars([(1, 'A'), (2, 'B')], &small).is_err());
        Ok(())
    }
}
