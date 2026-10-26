//! Positioned rich-text glyphs, sharing the reversible PDF font encoder.

use super::style::Style;
use super::xml::Text;
use crate::fonts::{Encoding, FontSource};
use crate::form::{da::DefaultAppearance, supply_font};
use crate::program::Program;
use crate::values::optional_dictionary;
use pagura_core::{Dictionary, Error, Limits, Meter, Name, Result, Value};
use pagura_document::{Document, forms::Justification};

pub(crate) struct Layout {
    text: Text,
    glyphs: Vec<Glyph>,
    state: Vec<u8>,
    matrix: [f64; 4],
    pub(crate) fonts: Dictionary,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

struct Glyph {
    code: u8,
    font: usize,
    style: usize,
    advance: f64,
    ascent: f64,
    descent: f64,
    x: f64,
    y: f64,
    drawn: bool,
}

struct Font {
    family: Name,
    encoding: Encoding,
}

pub(super) fn prepare(
    document: &Document,
    text: Text,
    appearance: &DefaultAppearance,
    width: f64,
    source: &dyn FontSource,
    limits: &Limits,
    work: &mut Meter,
) -> Result<Layout> {
    let mut storage = Meter::new(limits.document_bytes, "rich text layout storage exceeded");
    let mut font_storage = Meter::new(limits.document_bytes, "rich text font storage exceeded");
    let mut fonts: Vec<Font> = Vec::new();
    let mut entries = Dictionary::new();
    let mut glyphs = Vec::new();
    for character in &text.characters {
        work.charge(1)?;
        let style = &text.styles[character.style];
        let family = font_name(style);
        work.charge(fonts.len())?;
        let index = match fonts
            .iter()
            .position(|font| font.family.as_bytes() == family.as_bytes())
        {
            Some(index) => index,
            None => {
                font_storage.charge(256 * 64)?;
                work.charge(256)?;
                let (entry, resources) = font_resources(document, family.as_bytes())?;
                font_storage.charge(entry.retained_bytes())?;
                font_storage.charge(resources.retained_bytes())?;
                let encoding = Encoding::load(document, &resources, family.as_bytes(), source)?;
                let index = fonts.len();
                font_storage.reserve_capacity(&mut fonts, 1)?;
                entries.insert(Name::new(format!("Rich{index}").as_bytes()), entry);
                fonts.push(Font {
                    family: Name::new(family.as_bytes()),
                    encoding,
                });
                index
            }
        };
        let encoding = &fonts[index].encoding;
        let character_value = if matches!(character.value, '\n' | '\t') {
            ' '
        } else {
            character.value
        };
        let code = encoding.encode(character_value)?;
        let advance = (code.advance * style.size + style.spacing)
            * style.scale[0]
            * appearance.scale
            * appearance.matrix[0];
        let ascent =
            encoding.ascent * style.size * style.scale[1] * appearance.matrix[3] + style.rise;
        let descent =
            encoding.descent * style.size * style.scale[1] * appearance.matrix[3] + style.rise;
        if !advance.is_finite() || !ascent.is_finite() || !descent.is_finite() {
            return Err(Error::limit(
                "rich text glyph geometry exceeds numeric range",
            ));
        }
        storage.reserve_capacity(&mut glyphs, 1)?;
        glyphs.push(Glyph {
            code: code.code,
            font: index,
            style: character.style,
            advance,
            ascent,
            descent,
            x: 0.0,
            y: 0.0,
            drawn: false,
        });
    }
    storage.charge(appearance.state.len())?;
    let mut result = Layout {
        text,
        glyphs,
        state: appearance.state.clone(),
        matrix: appearance.matrix,
        fonts: entries,
        width: 0.0,
        height: 0.0,
    };
    result.position(width, work)?;
    Ok(result)
}

impl Layout {
    fn position(&mut self, width: f64, work: &mut Meter) -> Result<()> {
        let mut top = 0.0;
        for paragraph in &self.text.paragraphs {
            let style = &self.text.styles[paragraph.style];
            top += style.margin[0];
            let (mut start, end) = (paragraph.characters.start, paragraph.characters.end);
            let mut first = true;
            while start < end {
                let left = style.margin[3] + if first { style.indent } else { 0.0 };
                let available = width - left - style.margin[1];
                let (mut stop, mut measured, mut space) = (start, 0.0, None);
                while stop < end {
                    work.charge(1)?;
                    let c = self.text.characters[stop].value;
                    if c == '\n' {
                        break;
                    }
                    let next = measured + self.glyphs[stop].advance;
                    if next > available && stop > start {
                        if let Some((index, before)) = space {
                            stop = index;
                            measured = before;
                        }
                        break;
                    }
                    if c == ' ' {
                        space = Some((stop, measured));
                    }
                    measured = next;
                    stop += 1;
                }
                let row = self
                    .glyphs
                    .get(start..stop)
                    .ok_or(Error::syntax("invalid rich text row"))?;
                let ascent = row.iter().map(|glyph| glyph.ascent).fold(0.0, f64::max);
                let descent = row.iter().map(|glyph| glyph.descent).fold(0.0, f64::min);
                let leading = row
                    .iter()
                    .map(|glyph| {
                        let style = &self.text.styles[glyph.style];
                        style
                            .leading
                            .unwrap_or(style.size * 1.2 * style.scale[1] * self.matrix[3])
                    })
                    .fold(style.leading.unwrap_or(style.size * 1.2), f64::max);
                let baseline = top + ascent;
                let available = if available.is_finite() {
                    available
                } else {
                    measured
                };
                let mut x = left
                    + match style.alignment {
                        Justification::Left => 0.0,
                        Justification::Center => (available - measured) / 2.0,
                        Justification::Right => available - measured,
                    };
                for glyph in &mut self.glyphs[start..stop] {
                    glyph.x = x;
                    glyph.y = -baseline;
                    glyph.drawn = true;
                    x += glyph.advance;
                }
                self.width = self.width.max(left + measured + style.margin[1]);
                self.height = self.height.max(baseline - descent);
                top += leading;
                start = stop;
                if start < end && self.text.characters[start].value == '\n' {
                    start += 1;
                } else {
                    while start < end && self.text.characters[start].value == ' ' {
                        start += 1;
                        work.charge(1)?;
                    }
                }
                first = false;
            }
            if first {
                top += style.leading.unwrap_or(style.size * 1.2);
            }
            top += style.margin[2];
            self.height = self
                .height
                .max(top - style.leading.unwrap_or(style.size * 1.2) + style.margin[2]);
        }
        if !self.width.is_finite() || !self.height.is_finite() || !top.is_finite() {
            return Err(Error::limit("rich text layout exceeds numeric range"));
        }
        Ok(())
    }

    /// Writes relative to the top-left `origin`, with the supplied unit axis.
    pub(crate) fn write(
        &self,
        out: &mut Program,
        origin: (f64, f64),
        axis: (f64, f64),
    ) -> Result<()> {
        for glyph in self.glyphs.iter().filter(|glyph| glyph.drawn) {
            let style = &self.text.styles[glyph.style];
            let x = origin.0 + axis.0 * glyph.x - axis.1 * glyph.y;
            let y = origin.1 + axis.1 * glyph.x + axis.0 * glyph.y;
            let sx = style.scale[0] * self.matrix[0];
            let sy = style.scale[1] * self.matrix[3];
            let values = [axis.0 * sx, axis.1 * sx, -axis.1 * sy, axis.0 * sy, x, y];
            if values.iter().any(|v| !v.is_finite()) {
                return Err(Error::limit("rich text placement exceeds numeric range"));
            }
            out.text("q\nBT\n0 Tc\n0 Tw\n100 Tz\n0 TL\n0 Tr\n0 Ts\n")?;
            out.bytes(&self.state)?;
            if let Some([r, g, b]) = style.color {
                out.text(&format!("{r} {g} {b} rg\n"))?;
            }
            out.text(&format!(
                "/Rich{} {} Tf\n{} Ts\n{} {} {} {} {} {} Tm\n<{:02X}> Tj\nET\n",
                glyph.font,
                style.size,
                style.rise,
                values[0],
                values[1],
                values[2],
                values[3],
                values[4],
                values[5],
                glyph.code
            ))?;
            if style.decoration != 0 {
                out.text(&format!(
                    "{} {} {} {} {} {} cm\n",
                    axis.0, axis.1, -axis.1, axis.0, x, y
                ))?;
                let thickness = style.size / 20.0;
                for (flag, offset) in [
                    (1, -style.size / 10.0),
                    (2, style.size / 3.0),
                    (4, style.size * 0.8),
                ] {
                    if style.decoration & flag != 0 {
                        out.text(&format!("0 {offset} {} {thickness} re f\n", glyph.advance))?;
                    }
                }
            }
            out.text("Q\n")?;
        }
        Ok(())
    }
}

fn font_resources(document: &Document, name: &[u8]) -> Result<(Value, Dictionary)> {
    let mut resources = Dictionary::new();
    if let Some(form) = optional_dictionary(document, document.catalog()?, b"AcroForm")?
        && let Some(defaults) = optional_dictionary(document, form, b"DR")?
        && let Some(fonts) = optional_dictionary(document, defaults, b"Font")?
        && let Some(font) = fonts.get(name)
    {
        let mut entries = Dictionary::new();
        entries.insert(Name::new(name), font.clone());
        resources.insert(Name::new(b"Font"), Value::Dictionary(entries));
    }
    supply_font(document, &mut resources, name)?;
    let entry = resources
        .get(b"Font")
        .and_then(Value::as_dictionary)
        .and_then(|fonts| fonts.get(name))
        .ok_or(Error::syntax("missing rich text font"))?
        .clone();
    Ok((entry, resources))
}

fn font_name(style: &Style) -> String {
    let (base, bold, italic) = match style.family.as_str() {
        "sans-serif" | "Arial" | "Helvetica" => ("Helvetica", "Bold", "Oblique"),
        "serif" | "Times" | "Times New Roman" | "Times-Roman" => ("Times", "Bold", "Italic"),
        "monospace" | "Courier New" | "Courier" => ("Courier", "Bold", "Oblique"),
        _ => return style.family.clone(),
    };
    match (style.bold, style.italic) {
        (true, true) => format!("{base}-{bold}{italic}"),
        (true, false) => format!("{base}-{bold}"),
        (false, true) => format!("{base}-{italic}"),
        (false, false) => {
            if base == "Times" {
                "Times-Roman".into()
            } else {
                base.into()
            }
        }
    }
}
