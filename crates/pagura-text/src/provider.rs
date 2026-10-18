//! Host-supplied data absent from a PDF (ADR 0009).
use crate::{WritingMode, cmap::CMap};
use pagura_core::{Error, Limits, Name, Result, syntax::Origin};
use pagura_font::FontProgram;
use std::{collections::BTreeMap, sync::Arc};

/// External font resources. Implementations own their caches and budgets;
/// interpretation never opens files or acquires host capabilities.
/// Shared results are excluded from [`crate::PdfFont::retained_bytes`]; a
/// provider must budget them once in its own cache, independently of fonts.
/// The host must cover outstanding shared results even after cache eviction
/// or destruction of the provider itself.
pub trait Provider: Send + Sync {
    /// Standard-14 metrics and optional substitute, by canonical base name.
    ///
    /// # Errors
    /// Returns parsing or resource-budget errors; `None` means unavailable.
    fn standard_font(&self, base_name: &[u8]) -> Result<Option<Arc<StandardFont>>>;
    /// A predefined character-code to CID CMap (9.7.5.2, Table 116).
    ///
    /// # Errors
    /// Returns parsing or resource-budget errors; `None` means unavailable.
    fn predefined_cmap(&self, name: &[u8]) -> Result<Option<Arc<PredefinedCMap>>>;
    /// CID to Unicode data for a registry and ordering (9.10.2).
    ///
    /// # Errors
    /// Returns parsing or resource-budget errors; `None` means unavailable.
    fn cid_to_unicode(&self, registry: &[u8], ordering: &[u8])
    -> Result<Option<Arc<CidToUnicode>>>;
}

/// AFM font-wide descriptor values, in thousandths of text space.
#[derive(Debug, Clone, Default, PartialEq)]
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
    pub weight: String,
}

/// Standard-font metrics and its built-in code-to-name encoding (9.6.2.2).
#[derive(Debug, Clone)]
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
    /// Width of a code in the built-in encoding, in AFM units.
    pub fn width(&self, code: u8) -> Option<f64> {
        self.encoding[usize::from(code)]
            .as_ref()
            .and_then(|name| self.widths.get(name))
            .copied()
    }
}

/// Shared metrics and a substitute outline program. Missing outlines do not
/// prevent metric lookup (notably Symbol and ZapfDingbats).
#[derive(Debug, Clone)]
pub struct StandardFont {
    /// Original metrics and encoding; these determine layout.
    pub metrics: Arc<StandardMetrics>,
    /// Replacement outlines, whose own advances do not override PDF/AFM widths.
    pub program: Option<Arc<FontProgram>>,
}

/// A parsed, immutable predefined CMap, including resolved `usecmap` parents.
#[derive(Debug, Clone)]
pub struct PredefinedCMap(pub(crate) Arc<CMap>);

impl PredefinedCMap {
    /// Parses the declarative CMap subset of TN5014 sections 5.1–5.4.
    /// `parent` resolves a `usecmap` name; its implementation must bound graph
    /// depth and reject cycles. Child mappings override inherited mappings.
    ///
    /// # Errors
    /// Returns syntax, unsupported-operator, parent-resolution or limit errors.
    pub fn parse(
        bytes: &[u8],
        limits: &Limits,
        mut parent: impl FnMut(&[u8]) -> Result<Arc<Self>>,
    ) -> Result<Self> {
        if bytes.len() > limits.decoded_bytes {
            return Err(Error::limit("CMap resource byte budget exceeded"));
        }
        let mut map = CMap::parse(bytes, Origin::Buffer, *limits, false)?;
        let mut work = map.parse_work;
        if let Some(name) = &map.use_name {
            let base = parent(name.as_bytes())?;
            map.inherit_resource(base.0.as_ref().clone(), *limits, &mut work)?;
        }
        map.validate(*limits, &mut work)?;
        Ok(Self(Arc::new(map)))
    }

    /// Writing mode of this CMap.
    pub fn writing_mode(&self) -> WritingMode {
        self.0.mode
    }

    /// Registry and ordering from CIDSystemInfo, when present.
    pub fn collection(&self) -> Option<(&[u8], &[u8])> {
        Some((
            self.0.registry.as_ref()?.as_bytes(),
            self.0.ordering.as_ref()?.as_bytes(),
        ))
    }

    /// Decodes one code, returning `(code, byte_length, CID)` including notdef.
    ///
    /// # Errors
    /// Returns syntax errors for empty input and errors for exceeded budgets.
    pub fn decode_code(&self, bytes: &[u8], limits: &Limits) -> Result<(u32, u8, u32)> {
        if bytes.is_empty() {
            return Err(Error::syntax("empty CMap input"));
        }
        let mut work = 0;
        let (code, valid) = self.0.next(bytes, &mut work, *limits)?;
        let cid = if valid {
            self.0.cid(code, &mut work, *limits)?
        } else {
            None
        };
        Ok((
            code.value,
            code.len,
            cid.unwrap_or(self.0.notdef(code, &mut work, *limits)?),
        ))
    }
}

/// Shared CID-to-Unicode values for one character collection.
#[derive(Debug, Clone)]
pub struct CidToUnicode(BTreeMap<u32, String>);

impl CidToUnicode {
    /// Validates bounded, single-collection mapping data.
    ///
    /// # Errors
    /// Returns a limit error for excessive entries/output or syntax errors for
    /// CIDs outside the 16-bit PDF character-identifier range.
    pub fn new(values: BTreeMap<u32, String>, limits: &Limits) -> Result<Self> {
        let mut bytes = 0;
        if values.len() > limits.value_nodes {
            return Err(Error::limit("CID Unicode entry budget exceeded"));
        }
        for (cid, text) in &values {
            if *cid > 65535 {
                return Err(Error::syntax("CID outside 16-bit range"));
            }
            crate::cmap::charge(&mut bytes, text.len(), limits.decoded_bytes)?;
        }
        Ok(Self(values))
    }
    /// Unicode for a CID. Absence remains unknown rather than guessed.
    pub fn get(&self, cid: u32) -> Option<&str> {
        self.0.get(&cid).map(String::as_str)
    }
}
