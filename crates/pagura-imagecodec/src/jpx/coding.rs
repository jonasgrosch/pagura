//! Coding and quantization markers, T.800 A.6–A.7.

use pagura_core::bytes::Cursor;
use pagura_core::{Position, Result};

use crate::budget::Budget;

use super::marker::Marker;
use super::progression::{self, Change, ProgressionOrder};
use super::segments;
use super::{finish, syntax, unsupported};

/// `Scod` and `Scoc` flags (T.800 Tables A.13 and A.23).
mod scod {
    /// Precinct sizes follow; otherwise every precinct is 2^15 square.
    pub(super) const PRECINCTS: u8 = 1;
    /// Packets may start with SOP marker segments.
    pub(super) const SOP: u8 = 2;
    /// Packet headers end with EPH markers.
    pub(super) const EPH: u8 = 4;
}

/// Wavelet transformations (T.800 Table A.20).
const IRREVERSIBLE_9_7: u8 = 0;
const REVERSIBLE_5_3: u8 = 1;

/// Whether the first three components go through the multiple component
/// transformation (T.800 Table A.17).
const NO_TRANSFORM: u8 = 0;
const COMPONENT_TRANSFORM: u8 = 1;

#[derive(Clone, Default)]
pub(super) struct Style {
    pub(super) levels: u8,
    /// Code-block width and height exponents (T.800 Table A.18).
    pub(super) block: [u8; 2],
    /// Code-block style flags, the `segments` constants (T.800 Table A.19).
    pub(super) modes: u8,
    pub(super) reversible: bool,
    /// Precinct width and height exponents per resolution (T.800 Table A.21).
    pub(super) precincts: Vec<[u8; 2]>,
}

fn read_style(reader: &mut Cursor<'_>, flags: u8, budget: &mut Budget) -> Result<Style> {
    let start = reader.position();
    let levels = reader.u8()?;
    let block = [reader.u8()? & 15, reader.u8()? & 15];
    let modes = reader.u8()?;
    let transform = reader.u8()?;
    if levels > 32
        || block.iter().any(|&n| n > 8)
        || u16::from(block[0]) + u16::from(block[1]) > 8
        || modes & !segments::ALL != 0
        || !matches!(transform, IRREVERSIBLE_9_7 | REVERSIBLE_5_3)
    {
        return Err(syntax("invalid JPEG 2000 coding style", start));
    }
    let mut precincts = budget.filled(usize::from(levels) + 1, [15, 15])?;
    if flags & scod::PRECINCTS != 0 {
        for (resolution, exponents) in precincts.iter_mut().enumerate() {
            let at = reader.position();
            let value = reader.u8()?;
            *exponents = [value & 15, value >> 4];
            if resolution != 0 && exponents.contains(&0) {
                return Err(syntax(
                    "invalid JPEG 2000 high-resolution precinct size",
                    at,
                ));
            }
        }
    }
    budget.work(usize::from(levels) + 1)?;
    finish(reader)?;
    Ok(Style {
        levels,
        // T.800 Table A.18 stores the exponents less two.
        block: [block[0] + 2, block[1] + 2],
        modes,
        reversible: transform == REVERSIBLE_5_3,
        precincts,
    })
}

/// How the step sizes of the subbands are signalled (T.800 Table A.28).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum QuantizationStyle {
    /// No quantization, for reversible coding: one exponent per subband.
    #[default]
    None,
    /// Scalar derived: the LL step, from which the others follow (T.800 E-5).
    Derived,
    /// Scalar expounded: one step per subband.
    Expounded,
}

impl TryFrom<u8> for QuantizationStyle {
    type Error = u8;

    fn try_from(code: u8) -> std::result::Result<Self, u8> {
        match code {
            0 => Ok(Self::None),
            1 => Ok(Self::Derived),
            2 => Ok(Self::Expounded),
            _ => Err(code),
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct Quantization {
    pub(super) guard: u8,
    pub(super) style: QuantizationStyle,
    /// Exponent and mantissa per subband (T.800 Tables A.29 and A.30).
    pub(super) steps: Vec<(u8, u16)>,
}

fn read_quantization(reader: &mut Cursor<'_>, budget: &mut Budget) -> Result<Quantization> {
    let start = reader.position();
    // T.800 Table A.28: three guard bits above a five-bit style.
    let header = reader.u8()?;
    let guard = header >> 5;
    let style = QuantizationStyle::try_from(header & 31)
        .map_err(|_| syntax("invalid JPEG 2000 quantization style", start))?;
    let unit = if style == QuantizationStyle::None {
        1
    } else {
        2
    };
    let count = reader.remaining() / unit;
    if count == 0
        || count > 97
        || !reader.remaining().is_multiple_of(unit)
        || (style == QuantizationStyle::Derived && count != 1)
    {
        return Err(reader.error("invalid JPEG 2000 quantization step count"));
    }
    let mut steps = budget.with_capacity(count)?;
    for _ in 0..count {
        if style == QuantizationStyle::None {
            // T.800 Table A.29: a five-bit exponent above three reserved bits.
            let value = reader.u8()?;
            steps.push((value >> 3, 0));
        } else {
            // T.800 Table A.30: a five-bit exponent above an 11-bit mantissa.
            let [high, low] = reader.take_array()?;
            steps.push((high >> 3, u16::from_be_bytes([high & 7, low])));
        }
    }
    Ok(Quantization {
        guard,
        style,
        steps,
    })
}

/// The optional packet markers a coding style allows (T.800 Table A.13).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct PacketMarkers {
    /// Packets may start with an SOP marker segment.
    pub(super) sop: bool,
    /// Packet headers end with an EPH marker.
    pub(super) eph: bool,
}

/// The default marker segments one header has had; each may appear once
/// (T.800 A.6).
#[derive(Clone, Copy, Default)]
struct Defaults {
    cod: bool,
    qcd: bool,
    poc: bool,
}

/// The component marker segments one header has had for a component; each
/// may appear once per component (T.800 A.6).
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct ComponentMarkers(u8);

impl ComponentMarkers {
    const COC: Self = Self(1);
    const QCC: Self = Self(2);
    const RGN: Self = Self(4);

    const fn contains(self, marker: Self) -> bool {
        self.0 & marker.0 != 0
    }

    const fn insert(&mut self, marker: Self) {
        self.0 |= marker.0;
    }
}

/// The coding parameters of the main header or of one tile. The
/// per-component vectors have one entry per component of the image.
pub(super) struct Configuration {
    pub(super) styles: Vec<Style>,
    pub(super) quants: Vec<Quantization>,
    pub(super) progression: ProgressionOrder,
    pub(super) layers: usize,
    pub(super) transform: bool,
    pub(super) markers: PacketMarkers,
    pub(super) roi: Vec<u8>,
    pub(super) progressions: Vec<Change>,
    has_style: bool,
    has_quant: bool,
    seen: Defaults,
    seen_components: Vec<ComponentMarkers>,
}

impl Configuration {
    pub(super) fn new(count: usize, budget: &mut Budget) -> Result<Self> {
        Ok(Self {
            styles: budget.filled(count, Style::default())?,
            quants: budget.filled(count, Quantization::default())?,
            progression: ProgressionOrder::default(),
            layers: 0,
            transform: false,
            markers: PacketMarkers::default(),
            roi: budget.filled(count, 0)?,
            progressions: Vec::new(),
            has_style: false,
            has_quant: false,
            seen: Defaults::default(),
            seen_components: budget.filled(count, ComponentMarkers::default())?,
        })
    }

    /// A copy for a tile's own header, which may override the defaults.
    pub(super) fn copy(&self, budget: &mut Budget) -> Result<Self> {
        copied::<Style>(budget, self.styles.len())?;
        copied::<Quantization>(budget, self.quants.len())?;
        for style in &self.styles {
            copied::<[u8; 2]>(budget, style.precincts.len())?;
        }
        copied::<u8>(budget, self.roi.len())?;
        copied::<Change>(budget, self.progressions.len())?;
        for q in &self.quants {
            copied::<(u8, u16)>(budget, q.steps.len())?;
        }
        Ok(Self {
            styles: self.styles.clone(),
            quants: self.quants.clone(),
            progression: self.progression,
            layers: self.layers,
            transform: self.transform,
            markers: self.markers,
            roi: self.roi.clone(),
            progressions: self.progressions.clone(),
            has_style: self.has_style,
            has_quant: self.has_quant,
            seen: Defaults::default(),
            seen_components: budget.filled(self.styles.len(), ComponentMarkers::default())?,
        })
    }

    /// Applies the marker segment `marker`, found at `position`, whose
    /// parameters `fields` holds.
    pub(super) fn marker(
        &mut self,
        marker: Marker,
        position: Position,
        fields: &mut Cursor<'_>,
        budget: &mut Budget,
    ) -> Result<()> {
        match marker {
            Marker::COD => self.coding_default(position, fields, budget)?,
            Marker::COC => {
                let component = self.component_marker(fields, ComponentMarkers::COC, position)?;
                let at = fields.position();
                let flags = fields.u8()?;
                if flags & !scod::PRECINCTS != 0 {
                    return Err(syntax("invalid JPEG 2000 component coding flags", at));
                }
                self.styles[component] = read_style(fields, flags, budget)?;
            }
            Marker::QCD => {
                once(&mut self.seen.qcd, position)?;
                let quant = read_quantization(fields, budget)?;
                for (entry, seen) in self.quants.iter_mut().zip(&self.seen_components) {
                    if seen.contains(ComponentMarkers::QCC) {
                        continue;
                    }
                    copied::<(u8, u16)>(budget, quant.steps.len())?;
                    entry.clone_from(&quant);
                }
                self.has_quant = true;
            }
            Marker::QCC => {
                let component = self.component_marker(fields, ComponentMarkers::QCC, position)?;
                self.quants[component] = read_quantization(fields, budget)?;
            }
            Marker::RGN => {
                let component = self.component_marker(fields, ComponentMarkers::RGN, position)?;
                let at = fields.position();
                // T.800 Table A.26: implicit (maxshift) is the only ROI style.
                if fields.u8()? != 0 {
                    return Err(syntax("invalid JPEG 2000 ROI style", at));
                }
                self.roi[component] = fields.u8()?;
                finish(fields)?;
            }
            Marker::POC => self.progression_marker(position, fields, budget)?,
            Marker::TLM => validate_tile_lengths(fields)?,
            Marker::PLT => validate_packet_lengths(fields, budget)?,
            Marker::COM => {
                // T.800 A.9.2: registration 0 is binary data, 1 Latin text.
                if fields.u16()? > 1 || fields.at_end() {
                    return Err(syntax(
                        "invalid JPEG 2000 comment registration or length",
                        position,
                    ));
                }
                fields.take_rest();
            }
            Marker::PLM | Marker::CRG => {
                return Err(unsupported(
                    "JPEG 2000 PLM and CRG markers are not implemented",
                    position,
                ));
            }
            _ => {
                return Err(unsupported(
                    "JPEG 2000 marker extension is not implemented",
                    position,
                ));
            }
        }
        Ok(())
    }

    /// Applies a COD marker segment (T.800 A.6.1).
    fn coding_default(
        &mut self,
        position: Position,
        fields: &mut Cursor<'_>,
        budget: &mut Budget,
    ) -> Result<()> {
        once(&mut self.seen.cod, position)?;
        let start = fields.position();
        let flags = fields.u8()?;
        let progression = fields.u8()?;
        self.layers = usize::from(fields.u16()?);
        let transform = fields.u8()?;
        let progression = ProgressionOrder::try_from(progression)
            .ok()
            .filter(|_| {
                flags & !(scod::PRECINCTS | scod::SOP | scod::EPH) == 0
                    && self.layers != 0
                    && matches!(transform, NO_TRANSFORM | COMPONENT_TRANSFORM)
            })
            .ok_or_else(|| syntax("invalid JPEG 2000 coding parameters", start))?;
        self.progression = progression;
        self.transform = transform == COMPONENT_TRANSFORM;
        self.markers = PacketMarkers {
            sop: flags & scod::SOP != 0,
            eph: flags & scod::EPH != 0,
        };
        let style = read_style(fields, flags, budget)?;
        for (entry, seen) in self.styles.iter_mut().zip(&self.seen_components) {
            if seen.contains(ComponentMarkers::COC) {
                continue;
            }
            copied::<[u8; 2]>(budget, style.precincts.len())?;
            entry.clone_from(&style);
        }
        self.has_style = true;
        Ok(())
    }

    fn progression_marker(
        &mut self,
        position: Position,
        fields: &mut Cursor<'_>,
        budget: &mut Budget,
    ) -> Result<()> {
        once(&mut self.seen.poc, position)?;
        let values = progression::changes(
            fields,
            self.styles.len(),
            if self.layers == 0 {
                usize::MAX
            } else {
                self.layers
            },
            budget,
        )?;
        // A tile's POC replaces the main header's (T.800 A.6.6).
        self.progressions.clear();
        for value in values {
            budget.push(&mut self.progressions, value)?;
        }
        Ok(())
    }

    /// Reads the component index of a component marker segment and records
    /// the segment for that component.
    fn component_marker(
        &mut self,
        fields: &mut Cursor<'_>,
        marker: ComponentMarkers,
        position: Position,
    ) -> Result<usize> {
        let at = fields.position();
        // T.800 Table A.23: one byte for up to 256 components, two above.
        let component = if self.seen_components.len() < 257 {
            usize::from(fields.u8()?)
        } else {
            usize::from(fields.u16()?)
        };
        let seen = self
            .seen_components
            .get_mut(component)
            .ok_or_else(|| syntax("JPEG 2000 component index out of range", at))?;
        if seen.contains(marker) {
            return Err(syntax("repeated JPEG 2000 component marker", position));
        }
        seen.insert(marker);
        Ok(component)
    }

    pub(super) const fn has_local_poc(&self) -> bool {
        self.seen.poc
    }

    /// Checks the configuration that takes effect at `position`: the end of
    /// the main header or the start of a tile.
    pub(super) fn validate(&self, position: Position) -> Result<()> {
        if !self.has_style || !self.has_quant {
            return Err(syntax(
                "missing JPEG 2000 coding or quantization defaults",
                position,
            ));
        }
        if self.progressions.iter().any(|p| p.layer_end > self.layers) {
            return Err(syntax(
                "JPEG 2000 progression exceeds tile layer count",
                position,
            ));
        }
        for (style, quant) in self.styles.iter().zip(&self.quants) {
            if (quant.style != QuantizationStyle::Derived
                && quant.steps.len() < 1 + 3 * usize::from(style.levels))
                || (style.reversible != (quant.style == QuantizationStyle::None))
            {
                return Err(syntax(
                    "JPEG 2000 quantization does not match coding style",
                    position,
                ));
            }
        }
        for (quant, &roi) in self.quants.iter().zip(&self.roi) {
            // T.800 E-2: Mb = G + εb - 1 bit planes.
            if quant.steps.iter().any(|&(exponent, _)| {
                u16::from(exponent) + u16::from(quant.guard) + u16::from(roi)
                    > u16::from(segments::MAX_PLANES) + 1
            }) {
                return Err(unsupported(
                    "JPEG 2000 coefficient precision exceeds 30 bit planes",
                    position,
                ));
            }
        }
        if self.transform
            && (self.styles.len() < 3
                || self.styles[..3]
                    .iter()
                    .any(|s| s.reversible != self.styles[0].reversible))
        {
            return Err(syntax(
                "invalid JPEG 2000 multiple component transform",
                position,
            ));
        }
        Ok(())
    }
}

/// Records a default marker segment, which one header may hold once.
fn once(seen: &mut bool, position: Position) -> Result<()> {
    if std::mem::replace(seen, true) {
        return Err(syntax(
            "repeated or misplaced JPEG 2000 default marker",
            position,
        ));
    }
    Ok(())
}

/// Checks a TLM marker segment (T.800 A.7.1).
fn validate_tile_lengths(fields: &mut Cursor<'_>) -> Result<()> {
    fields.u8()?; // Ztlm orders optional tables; decoding does not depend on them.
    let at = fields.position();
    // Stlm: the sizes of Ttlm (bits 4 and 5) and Ptlm (bit 6).
    let style = fields.u8()?;
    let tile_bytes = usize::from((style >> 4) & 3);
    if tile_bytes == 3 {
        return Err(syntax("invalid JPEG 2000 tile-length table style", at));
    }
    let length_bytes = if style & 0x40 == 0 { 2 } else { 4 };
    if !fields.remaining().is_multiple_of(tile_bytes + length_bytes) {
        return Err(fields.error("invalid JPEG 2000 tile-length table"));
    }
    if fields.at_end() {
        return Err(fields.error("empty JPEG 2000 tile-length table"));
    }
    while !fields.at_end() {
        let tile = match tile_bytes {
            0 => 0,
            1 => u16::from(fields.u8()?),
            _ => fields.u16()?,
        };
        if tile == 65535 || tile_bytes == 1 && tile == 255 {
            return Err(fields.error("invalid JPEG 2000 tile-length index"));
        }
        let length = if length_bytes == 2 {
            u32::from(fields.u16()?)
        } else {
            fields.u32()?
        };
        // The SOT marker segment alone is 12 bytes, and SOD follows it.
        if length < 14 {
            return Err(fields.error("invalid JPEG 2000 tile-length value"));
        }
    }
    Ok(())
}

/// Checks a PLT marker segment (T.800 A.7.3): lengths of seven bits per
/// byte, the high bit marking that more follow.
fn validate_packet_lengths(fields: &mut Cursor<'_>, budget: &mut Budget) -> Result<()> {
    fields.u8()?; // Zplt.
    if fields.at_end() {
        return Err(fields.error("empty JPEG 2000 packet-length table"));
    }
    let mut value = 0_u32;
    let mut continued = false;
    while !fields.at_end() {
        budget.work(1)?;
        let at = fields.position();
        let byte = fields.u8()?;
        value = value
            .checked_mul(128)
            .and_then(|v| v.checked_add(u32::from(byte & 127)))
            .ok_or_else(|| syntax("JPEG 2000 packet-length table overflows", at))?;
        continued = byte & 128 != 0;
        if !continued {
            value = 0;
        }
    }
    if continued {
        return Err(fields.error("truncated JPEG 2000 packet-length table"));
    }
    Ok(())
}

/// Charges the working memory of `count` copied values of `T`, and the copying
/// as work.
fn copied<T>(budget: &mut Budget, count: usize) -> Result<()> {
    budget.memory::<T>(count)?;
    budget.work(count)
}
