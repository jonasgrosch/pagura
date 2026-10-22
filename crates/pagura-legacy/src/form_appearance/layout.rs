//! Form layout policy: whitespace wrapping, 1.2 em leading and a 12 pt auto-size cap.
//! ISO 32000-2, 12.7.4.3 leaves auto-size and detailed line layout to the reader.
use super::{Program, da::DefaultAppearance, font::Font};
use pagura_core::{Error, Limits, Rect, Result};
use pagura_document::forms::Justification;
use std::ops::Range;

#[derive(Clone, Copy)]
struct Character {
    code: u8,
    advance: f64,
    space: bool,
    newline: bool,
}
struct Line {
    range: Range<usize>,
    width: f64,
}
pub(super) struct TextLayout<'a> {
    pub text: &'a str,
    pub multiline: bool,
    pub comb: Option<usize>,
    pub password: bool,
    pub quadding: Justification,
    pub no_scroll: bool,
}
pub(super) fn write(
    out: &mut Program,
    font: &Font,
    da: &DefaultAppearance,
    area: Rect,
    layout: &TextLayout<'_>,
    limits: Limits,
) -> Result<()> {
    let chars = encode(font, layout.text, layout.multiline, layout.password, limits)?;
    let mut work = 0usize;
    let size = if da.size == 0.0 {
        auto_size(&chars, font, da, area, layout, limits, &mut work)?
    } else {
        da.size
    };
    let lines = lines(
        &chars,
        da,
        size,
        area.width(),
        layout.multiline,
        limits,
        &mut work,
    )?;
    let leading = leading(da, size);
    let height = (font.ascent - font.descent) * size * da.matrix[3]
        + (lines.len().saturating_sub(1) as f64) * leading * da.matrix[3];
    if layout.no_scroll
        && (height > area.height() + 1e-9
            || lines.iter().any(|line| line.width > area.width() + 1e-9))
    {
        return Err(Error::invalid_argument(
            "DoNotScroll field text does not fit its widget",
        ));
    }
    // BT does not reset text state (9.4.1); an old appearance can leave it set.
    out.text("0 Tc\n0 Tw\n100 Tz\n0 TL\n0 Tr\n0 Ts\n0 g\n")?;
    out.bytes(&da.state)?;
    out.text(&format!("{} {size} Tf\n", da.font))?;
    let baseline = if layout.multiline {
        area.y1() - font.ascent * size * da.matrix[3]
    } else {
        area.y0() + (area.height() - (font.ascent - font.descent) * size * da.matrix[3]) / 2.0
            - font.descent * size * da.matrix[3]
    };
    if let Some(cells) = layout.comb {
        if cells == 0 || chars.len() > cells {
            return Err(Error::invalid_argument("invalid Comb cell count"));
        }
        let cell = area.width() / cells as f64;
        let offset = match layout.quadding {
            Justification::Left => 0.0,
            Justification::Center => ((cells - chars.len()) / 2) as f64,
            Justification::Right => (cells - chars.len()) as f64,
        };
        for (i, c) in chars.iter().enumerate() {
            let width = advance(*c, da, size);
            let x = area.x0() + (offset + i as f64) * cell + (cell - width) / 2.0;
            matrix(out, da, x, baseline)?;
            out.text(&format!("<{:02X}> Tj\n", c.code))?;
        }
    } else {
        for (i, line) in lines.iter().enumerate() {
            let x = area.x0()
                + match layout.quadding {
                    Justification::Left => 0.0,
                    Justification::Center => (area.width() - line.width) / 2.0,
                    Justification::Right => area.width() - line.width,
                };
            matrix(out, da, x, baseline - i as f64 * leading * da.matrix[3])?;
            out.text("<")?;
            for c in &chars[line.range.clone()] {
                out.text(&format!("{:02X}", c.code))?;
            }
            out.text("> Tj\n")?;
        }
    }
    Ok(())
}
fn encode(
    font: &Font,
    text: &str,
    multiline: bool,
    password: bool,
    limits: Limits,
) -> Result<Vec<Character>> {
    if text.len() > limits.string_bytes
        || text.len().saturating_mul(size_of::<Character>()) > limits.document_bytes
    {
        return Err(Error::limit("appearance text storage budget exceeded"));
    }
    let mut chars = Vec::new();
    let mut input = text.chars().peekable();
    while let Some(c) = input.next() {
        if chars.len() >= limits.value_nodes {
            return Err(Error::limit("appearance character budget exceeded"));
        }
        let c = if c == '\r' {
            if input.peek() == Some(&'\n') {
                input.next();
            }
            '\n'
        } else {
            c
        };
        if c == '\n' && multiline {
            chars.push(Character {
                code: 0,
                advance: 0.0,
                space: false,
                newline: true,
            });
        } else {
            let c = if password {
                '*'
            } else if c == '\t' {
                ' '
            } else {
                c
            };
            let (code, advance) = font.encode(c)?;
            chars.push(Character {
                code,
                advance,
                space: c == ' ',
                newline: false,
            });
        }
    }
    Ok(chars)
}
fn matrix(out: &mut Program, da: &DefaultAppearance, x: f64, y: f64) -> Result<()> {
    let [a, b, c, d] = da.matrix;
    let y = y - da.rise * d;
    if !x.is_finite() || !y.is_finite() {
        return Err(Error::limit("appearance text positioning overflow"));
    }
    out.text(&format!("{a} {b} {c} {d} {x} {y} Tm\n"))
}
fn advance(c: Character, da: &DefaultAppearance, size: f64) -> f64 {
    (c.advance * size + da.character_space + if c.code == 32 { da.word_space } else { 0.0 })
        * da.scale
        * da.matrix[0]
}
fn leading(da: &DefaultAppearance, size: f64) -> f64 {
    if da.leading == 0.0 {
        size * 1.2
    } else {
        da.leading
    }
}
fn lines(
    chars: &[Character],
    da: &DefaultAppearance,
    size: f64,
    width: f64,
    wrap: bool,
    limits: Limits,
    work: &mut usize,
) -> Result<Vec<Line>> {
    let mut result = Vec::new();
    let (mut start, mut i, mut measured, mut last_space) = (0, 0, 0.0, None);
    while let Some(c) = chars.get(i) {
        if *work >= limits.operations {
            return Err(Error::limit("appearance layout work budget exceeded"));
        }
        *work += 1;
        if c.newline {
            result.push(Line {
                range: start..i,
                width: measured,
            });
            i += 1;
            start = i;
            measured = 0.0;
            last_space = None;
            continue;
        }
        let next = measured + advance(*c, da, size);
        if !next.is_finite() {
            return Err(Error::limit("appearance text width overflow"));
        }
        if wrap && next > width && i > start {
            let (end, line_width) = last_space.unwrap_or((i, measured));
            result.push(Line {
                range: start..end,
                width: line_width,
            });
            start = end;
            while chars.get(start).is_some_and(|c| c.space) {
                start += 1;
            }
            i = start;
            measured = 0.0;
            last_space = None;
            continue;
        }
        if c.space && i > start {
            last_space = Some((i, measured));
        }
        measured = next;
        i += 1;
    }
    result.push(Line {
        range: start..i,
        width: measured,
    });
    Ok(result)
}
fn auto_size(
    chars: &[Character],
    font: &Font,
    da: &DefaultAppearance,
    area: Rect,
    layout: &TextLayout<'_>,
    limits: Limits,
    work: &mut usize,
) -> Result<f64> {
    let cap = 12.0_f64.min(area.height() / ((font.ascent - font.descent) * da.matrix[3]));
    let fits = |size, work: &mut usize| -> Result<bool> {
        if let Some(cells) = layout.comb {
            if cells == 0 || chars.len() > cells {
                return Err(Error::invalid_argument("invalid Comb cell count"));
            }
            return Ok(chars
                .iter()
                .all(|c| advance(*c, da, size) <= area.width() / cells as f64));
        }
        let lines = lines(
            chars,
            da,
            size,
            area.width(),
            layout.multiline,
            limits,
            work,
        )?;
        Ok(lines.iter().all(|line| line.width <= area.width())
            && (font.ascent - font.descent) * size * da.matrix[3]
                + lines.len().saturating_sub(1) as f64 * leading(da, size) * da.matrix[3]
                <= area.height())
    };
    if fits(cap, work)? {
        return Ok(cap);
    }
    let (mut low, mut high) = (0.0, cap);
    for _ in 0..24 {
        let middle = f64::midpoint(low, high);
        if fits(middle, work)? {
            low = middle;
        } else {
            high = middle;
        }
    }
    let size = (low * 64.0).floor() / 64.0;
    if size < 1.0 / 64.0 {
        return Err(Error::invalid_argument(
            "field text cannot fit at the minimum auto-size",
        ));
    }
    Ok(size)
}
