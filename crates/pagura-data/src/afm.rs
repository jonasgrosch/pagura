//! Bounded parser for the pinned Core14 AFM files, not a PostScript interpreter.
use pagura_core::{Error, Limits, Name, Result};
use pagura_text::{StandardDescriptor, StandardMetrics};
use std::collections::BTreeMap;

pub(crate) fn parse(data: &[u8], limits: Limits) -> Result<StandardMetrics> {
    if data.len() > limits.decoded_bytes {
        return Err(Error::limit("AFM byte budget exceeded"));
    }
    let text = std::str::from_utf8(data).map_err(|_| Error::syntax("AFM is not UTF-8"))?;
    if !text.starts_with("StartFontMetrics ") || !text.trim_end().ends_with("EndFontMetrics") {
        return Err(Error::syntax("invalid AFM framing"));
    }
    let mut metrics = StandardMetrics {
        name: Name::new(b""),
        encoding: std::array::from_fn(|_| None),
        widths: BTreeMap::new(),
        descriptor: StandardDescriptor::default(),
    };
    let mut count = None;
    let mut remaining = 0;
    for (index, line) in text.lines().enumerate() {
        if index >= limits.operations {
            return Err(Error::limit("AFM operation budget exceeded"));
        }
        let line = line.trim();
        let (key, value) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let value = value.trim();
        if key == "EndCharMetrics" {
            if count.is_none() || remaining != 0 {
                return Err(Error::syntax("AFM character count mismatch"));
            }
            count = None;
        } else if count.is_some() {
            if remaining == 0 {
                return Err(Error::syntax("AFM character count mismatch"));
            }
            character(line, &mut metrics, limits)?;
            remaining -= 1;
        } else {
            descriptor(key, value, &mut metrics, limits)?;
            if key == "StartCharMetrics" {
                remaining = value
                    .parse::<usize>()
                    .map_err(|_| Error::syntax("invalid AFM character count"))?;
                if remaining > limits.value_nodes {
                    return Err(Error::limit("AFM glyph budget exceeded"));
                }
                count = Some(remaining);
            }
        }
    }
    if count.is_some() || metrics.name.is_empty() || metrics.widths.is_empty() {
        return Err(Error::syntax("incomplete AFM metrics"));
    }
    Ok(metrics)
}

fn descriptor(key: &str, value: &str, metrics: &mut StandardMetrics, limits: Limits) -> Result<()> {
    let d = &mut metrics.descriptor;
    match key {
        "FontName" => {
            bounded_name(value, limits)?;
            metrics.name = Name::new(value.as_bytes());
        }
        "FontBBox" => {
            let mut fields = value.split_whitespace();
            for slot in &mut d.bbox {
                *slot = number(
                    fields
                        .next()
                        .ok_or_else(|| Error::syntax("incomplete AFM bbox"))?,
                )?;
            }
            if fields.next().is_some() || d.bbox[0] > d.bbox[2] || d.bbox[1] > d.bbox[3] {
                return Err(Error::syntax("invalid AFM bbox"));
            }
        }
        "ItalicAngle" => d.italic_angle = number(value)?,
        "IsFixedPitch" => {
            d.fixed_pitch = match value {
                "true" => true,
                "false" => false,
                _ => return Err(Error::syntax("invalid AFM pitch")),
            }
        }
        "Weight" => {
            bounded_name(value, limits)?;
            value.clone_into(&mut d.weight);
        }
        "Ascender" => d.ascent = Some(number(value)?),
        "Descender" => d.descent = Some(number(value)?),
        "CapHeight" => d.cap_height = Some(number(value)?),
        "XHeight" => d.x_height = Some(number(value)?),
        "StdVW" => d.stem_v = Some(number(value)?),
        "StdHW" => d.stem_h = Some(number(value)?),
        _ => {}
    }
    Ok(())
}

fn character(line: &str, metrics: &mut StandardMetrics, limits: Limits) -> Result<()> {
    if metrics.widths.len() >= limits.value_nodes {
        return Err(Error::limit("AFM glyph budget exceeded"));
    }
    let mut code = None;
    let mut width = None;
    let mut name = None;
    for field in line.split(';') {
        let mut parts = field.split_whitespace();
        match parts.next() {
            Some("C") => {
                code = Some(
                    parts
                        .next()
                        .ok_or_else(|| Error::syntax("missing AFM code"))?
                        .parse::<i16>()
                        .map_err(|_| Error::syntax("invalid AFM code"))?,
                );
            }
            Some("WX") => {
                width = Some(number(
                    parts
                        .next()
                        .ok_or_else(|| Error::syntax("missing AFM width"))?,
                )?);
            }
            Some("N") => {
                let value = parts
                    .next()
                    .ok_or_else(|| Error::syntax("missing AFM name"))?;
                bounded_name(value, limits)?;
                name = Some(Name::new(value.as_bytes()));
            }
            _ => {}
        }
    }
    let code = code
        .filter(|code| (-1..=255).contains(code))
        .ok_or_else(|| Error::syntax("invalid AFM code"))?;
    let name = name.ok_or_else(|| Error::syntax("missing AFM name"))?;
    let width = width.ok_or_else(|| Error::syntax("missing AFM width"))?;
    if let Ok(code) = u8::try_from(code)
        && metrics.encoding[usize::from(code)]
            .replace(name.clone())
            .is_some()
    {
        return Err(Error::syntax("duplicate AFM code"));
    }
    if metrics.widths.insert(name, width).is_some() {
        return Err(Error::syntax("duplicate AFM glyph name"));
    }
    Ok(())
}

fn number(value: &str) -> Result<f64> {
    value
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| Error::syntax("invalid AFM number"))
}
fn bounded_name(value: &str, limits: Limits) -> Result<()> {
    if value.is_empty() || value.len() > limits.string_bytes {
        return Err(Error::limit("AFM name budget exceeded"));
    }
    Ok(())
}
