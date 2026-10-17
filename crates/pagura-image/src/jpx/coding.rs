//! Coding and quantization marker parameters (ISO/IEC 15444-1 Annex A).

use pagura_core::Result;

use super::io::{Budget, Reader, syntax};
use super::unsupported;

#[derive(Clone, Default)]
pub(super) struct Style {
    pub levels: u8,
    pub block: [u8; 2],
    pub modes: u8,
    pub reversible: bool,
}

fn read_style(reader: &mut Reader<'_>, flags: u8, budget: &mut Budget<'_>) -> Result<Style> {
    let levels = reader.u8()?;
    let block = [reader.u8()?, reader.u8()?];
    let modes = reader.u8()?;
    let transform = reader.u8()?;
    if levels > 32
        || block.iter().any(|&n| n > 8)
        || u16::from(block[0]) + u16::from(block[1]) > 8
        || modes & 0xc0 != 0
        || transform > 1
    {
        return Err(syntax(
            "invalid JPEG 2000 coding style",
            reader.position() - 5,
        ));
    }
    if modes & (1 | 4 | 8 | 16) != 0 {
        return Err(unsupported(
            "JPEG 2000 bypass, termination and causal code-block modes are not implemented",
            reader.position() - 2,
        ));
    }
    if flags & 1 != 0 {
        // Validate the field even when it cannot be used by this packet iterator.
        let precincts = reader.take(usize::from(levels) + 1)?;
        if precincts
            .iter()
            .skip(1)
            .any(|&p| p.trailing_zeros() >= 4 || p >> 4 == 0)
        {
            return Err(syntax(
                "invalid JPEG 2000 high-resolution precinct size",
                reader.position(),
            ));
        }
        if precincts.iter().any(|&p| p != 255) {
            return Err(unsupported(
                "JPEG 2000 explicit precinct partitioning is not implemented",
                reader.position(),
            ));
        }
    }
    budget.work(usize::from(levels) + 1)?;
    reader.finish()?;
    Ok(Style {
        levels,
        block: [block[0] + 2, block[1] + 2],
        modes,
        reversible: transform == 1,
    })
}

#[derive(Clone, Default)]
pub(super) struct Quantization {
    pub guard: u8,
    pub kind: u8,
    pub steps: Vec<(u8, u16)>,
}

fn read_quantization(reader: &mut Reader<'_>, budget: &mut Budget<'_>) -> Result<Quantization> {
    let header = reader.u8()?;
    let guard = header >> 5;
    let kind = header & 31;
    if kind > 2 {
        return Err(syntax(
            "invalid JPEG 2000 quantization style",
            reader.position() - 1,
        ));
    }
    let unit = if kind == 0 { 1 } else { 2 };
    let count = reader.remaining() / unit;
    if count == 0
        || count > 97
        || !reader.remaining().is_multiple_of(unit)
        || (kind == 1 && count != 1)
    {
        return Err(syntax(
            "invalid JPEG 2000 quantization step count",
            reader.position(),
        ));
    }
    budget.charge::<(u8, u16)>(count)?;
    let mut steps = Vec::with_capacity(count);
    for _ in 0..count {
        if kind == 0 {
            let value = reader.u8()?;
            if value & 7 != 0 {
                return Err(syntax(
                    "invalid reversible JPEG 2000 quantization step",
                    reader.position() - 1,
                ));
            }
            steps.push((value >> 3, 0));
        } else {
            let value = reader.u16()?;
            steps.push((
                u8::try_from(value >> 11)
                    .map_err(|_| syntax("JPEG 2000 exponent overflow", reader.position()))?,
                value & 2047,
            ));
        }
    }
    Ok(Quantization { guard, kind, steps })
}

pub(super) struct Configuration {
    pub styles: Vec<Style>,
    pub quants: Vec<Quantization>,
    pub progression: u8,
    pub layers: usize,
    pub transform: bool,
    pub markers: [bool; 2],
    has_style: bool,
    has_quant: bool,
    seen_defaults: [bool; 2],
    seen_components: Vec<u8>,
}

impl Configuration {
    pub fn new(count: usize, budget: &mut Budget<'_>) -> Result<Self> {
        Ok(Self {
            styles: budget.filled(count, Style::default())?,
            quants: budget.filled(count, Quantization::default())?,
            progression: 0,
            layers: 0,
            transform: false,
            markers: [false; 2],
            has_style: false,
            has_quant: false,
            seen_defaults: [false; 2],
            seen_components: budget.filled(count, 0)?,
        })
    }

    pub fn copy(&self, budget: &mut Budget<'_>) -> Result<Self> {
        budget.charge::<Style>(self.styles.len())?;
        budget.charge::<Quantization>(self.quants.len())?;
        for q in &self.quants {
            budget.charge::<(u8, u16)>(q.steps.len())?;
        }
        Ok(Self {
            styles: self.styles.clone(),
            quants: self.quants.clone(),
            progression: self.progression,
            layers: self.layers,
            transform: self.transform,
            markers: self.markers,
            has_style: self.has_style,
            has_quant: self.has_quant,
            seen_defaults: [false; 2],
            seen_components: budget.filled(self.styles.len(), 0)?,
        })
    }

    pub fn marker(
        &mut self,
        marker: u16,
        fields: &mut Reader<'_>,
        budget: &mut Budget<'_>,
    ) -> Result<()> {
        match marker {
            0xff52 => {
                self.default_marker(0, 1, fields.position())?;
                let flags = fields.u8()?;
                self.progression = fields.u8()?;
                self.layers = usize::from(fields.u16()?);
                let transform = fields.u8()?;
                if flags & !7 != 0 || self.progression > 4 || self.layers == 0 || transform > 1 {
                    return Err(syntax(
                        "invalid JPEG 2000 coding parameters",
                        fields.position(),
                    ));
                }
                self.transform = transform == 1;
                self.markers = [flags & 2 != 0, flags & 4 != 0];
                let style = read_style(fields, flags, budget)?;
                self.styles.fill(style);
                self.has_style = true;
            }
            0xff53 => {
                if !self.has_style {
                    return Err(syntax("JPEG 2000 COC precedes COD", fields.position()));
                }
                let component = component_index(fields, self.styles.len())?;
                self.component_marker(component, 1, fields.position())?;
                let flags = fields.u8()?;
                if flags > 1 {
                    return Err(syntax(
                        "invalid JPEG 2000 component coding flags",
                        fields.position() - 1,
                    ));
                }
                self.styles[component] = read_style(fields, flags, budget)?;
            }
            0xff5c => {
                self.default_marker(1, 2, fields.position())?;
                let quant = read_quantization(fields, budget)?;
                for entry in &mut self.quants {
                    budget.charge::<(u8, u16)>(quant.steps.len())?;
                    *entry = quant.clone();
                }
                self.has_quant = true;
            }
            0xff5d => {
                if !self.has_quant {
                    return Err(syntax("JPEG 2000 QCC precedes QCD", fields.position()));
                }
                let component = component_index(fields, self.styles.len())?;
                self.component_marker(component, 2, fields.position())?;
                self.quants[component] = read_quantization(fields, budget)?;
            }
            0xff5e => {
                let component = component_index(fields, self.styles.len())?;
                self.component_marker(component, 4, fields.position())?;
                if fields.u8()? != 0 {
                    return Err(syntax("invalid JPEG 2000 ROI style", fields.position() - 1));
                }
                if fields.u8()? != 0 {
                    return Err(unsupported(
                        "JPEG 2000 ROI maxshift is not implemented",
                        fields.position() - 1,
                    ));
                }
                fields.finish()?;
            }
            0xff5f => {
                return Err(unsupported(
                    "JPEG 2000 progression changes are not implemented",
                    fields.position(),
                ));
            }
            0xff60 | 0xff61 => {
                return Err(unsupported(
                    "JPEG 2000 packed packet headers are not implemented",
                    fields.position(),
                ));
            }
            0xff55 => validate_tile_lengths(fields)?,
            0xff58 => validate_packet_lengths(fields, budget)?,
            0xff64 => {
                fields.u16()?;
                fields.take(fields.remaining())?;
            }
            _ => {
                return Err(unsupported(
                    "JPEG 2000 marker extension is not implemented",
                    fields.position() - 4,
                ));
            }
        }
        Ok(())
    }

    fn default_marker(&mut self, index: usize, component_flag: u8, position: usize) -> Result<()> {
        if self.seen_defaults[index]
            || self
                .seen_components
                .iter()
                .any(|&v| v & component_flag != 0)
        {
            return Err(syntax(
                "repeated or misplaced JPEG 2000 default marker",
                position,
            ));
        }
        self.seen_defaults[index] = true;
        Ok(())
    }

    fn component_marker(&mut self, index: usize, flag: u8, position: usize) -> Result<()> {
        if self.seen_components[index] & flag != 0 {
            return Err(syntax("repeated JPEG 2000 component marker", position));
        }
        self.seen_components[index] |= flag;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if !self.has_style || !self.has_quant {
            return Err(syntax(
                "missing JPEG 2000 coding or quantization defaults",
                0,
            ));
        }
        for (style, quant) in self.styles.iter().zip(&self.quants) {
            if (quant.kind != 1 && quant.steps.len() != 1 + 3 * usize::from(style.levels))
                || (style.reversible && quant.kind != 0)
            {
                return Err(syntax(
                    "JPEG 2000 quantization does not match coding style",
                    0,
                ));
            }
        }
        if self.transform
            && (self.styles.len() < 3
                || self.styles[..3]
                    .iter()
                    .any(|s| s.reversible != self.styles[0].reversible))
        {
            return Err(syntax("invalid JPEG 2000 multiple component transform", 0));
        }
        Ok(())
    }
}

fn validate_tile_lengths(fields: &mut Reader<'_>) -> Result<()> {
    fields.u8()?; // Ztlm orders optional tables; decoding does not depend on them.
    let style = fields.u8()?;
    let tile_bytes = usize::from((style >> 4) & 3);
    if style & 0x8f != 0 || tile_bytes == 3 {
        return Err(syntax(
            "invalid JPEG 2000 tile-length table style",
            fields.position() - 1,
        ));
    }
    let length_bytes = if style & 0x40 == 0 { 2 } else { 4 };
    if !fields.remaining().is_multiple_of(tile_bytes + length_bytes) {
        return Err(syntax(
            "invalid JPEG 2000 tile-length table",
            fields.position(),
        ));
    }
    fields.take(fields.remaining())?;
    Ok(())
}

fn validate_packet_lengths(fields: &mut Reader<'_>, budget: &mut Budget<'_>) -> Result<()> {
    fields.u8()?; // Zplt.
    let mut value = 0_u32;
    let mut continued = false;
    while fields.remaining() != 0 {
        budget.work(1)?;
        let byte = fields.u8()?;
        value = value
            .checked_mul(128)
            .and_then(|v| v.checked_add(u32::from(byte & 127)))
            .ok_or_else(|| {
                syntax(
                    "JPEG 2000 packet-length table overflows",
                    fields.position() - 1,
                )
            })?;
        continued = byte & 128 != 0;
        if !continued {
            value = 0;
        }
    }
    if continued {
        return Err(syntax(
            "truncated JPEG 2000 packet-length table",
            fields.position(),
        ));
    }
    Ok(())
}

fn component_index(reader: &mut Reader<'_>, count: usize) -> Result<usize> {
    let index = if count < 257 {
        usize::from(reader.u8()?)
    } else {
        usize::from(reader.u16()?)
    };
    if index >= count {
        return Err(syntax(
            "JPEG 2000 component index out of range",
            reader.position(),
        ));
    }
    Ok(index)
}
