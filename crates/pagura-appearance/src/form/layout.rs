//! Form layout policy: whitespace wrapping, 1.2 em leading and a 12 pt
//! auto-size cap. 12.7.4.3 leaves auto-size and detailed line layout to the
//! reader.

use super::{Font, da::DefaultAppearance};
use crate::program::Program;
use pagura_core::{Error, Limits, Rect, Result};
use pagura_document::forms::Justification;
use std::ops::Range;

/// The text of one field or row and how to lay it out.
pub(crate) struct TextLayout<'a> {
    pub(crate) text: &'a str,
    pub(crate) multiline: bool,
    pub(crate) comb: Option<usize>,
    pub(crate) password: bool,
    pub(crate) quadding: Justification,
    pub(crate) no_scroll: bool,
}

/// Writes the text state and the positioned strings of `layout` in `area`;
/// the caller wraps them in `BT` and `ET`.
pub(crate) fn write(
    out: &mut Program,
    font: &Font,
    da: &DefaultAppearance,
    area: Rect,
    layout: &TextLayout<'_>,
    limits: &Limits,
) -> Result<()> {
    let chars = encode(font, layout, limits)?;
    let mut work = 0usize;
    let size = if da.size == 0.0 {
        let mut sizing = Sizing {
            chars: &chars,
            font,
            da,
            area,
            layout,
            limits,
            work: &mut work,
        };
        sizing.auto_size()?
    } else {
        da.size
    };
    let lines = lines(
        &chars,
        &Measure {
            da,
            size,
            width: area.width(),
            wrap: layout.multiline,
        },
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
        let line = Baseline {
            da,
            size,
            area,
            y: baseline,
        };
        return comb(out, &chars, cells, &line, layout.quadding);
    }
    for (i, line) in lines.iter().enumerate() {
        let x = area.x0()
            + match layout.quadding {
                Justification::Left => 0.0,
                Justification::Center => (area.width() - line.width) / 2.0,
                Justification::Right => area.width() - line.width,
            };
        matrix(out, da, x, baseline - i as f64 * leading * da.matrix[3])?;
        out.text("<")?;
        for c in chars.get(line.range.clone()).unwrap_or_default() {
            out.text(&format!("{:02X}", c.code))?;
        }
        out.text("> Tj\n")?;
    }
    Ok(())
}

/// A line of text: its box, baseline and font size.
struct Baseline<'a> {
    da: &'a DefaultAppearance,
    size: f64,
    area: Rect,
    y: f64,
}

/// One character per cell, centred in its cell; the run starts at the left,
/// centre or right cell by quadding (12.7.5.3, Comb).
fn comb(
    out: &mut Program,
    chars: &[Character],
    cells: usize,
    line: &Baseline<'_>,
    quadding: Justification,
) -> Result<()> {
    let Baseline { da, size, area, y } = *line;
    let spare = cells
        .checked_sub(chars.len())
        .filter(|_| cells != 0)
        .ok_or(Error::invalid_argument("invalid Comb cell count"))?;
    let cell = area.width() / cells as f64;
    let offset = match quadding {
        Justification::Left => 0.0,
        Justification::Center => (spare / 2) as f64,
        Justification::Right => spare as f64,
    };
    for (i, c) in chars.iter().enumerate() {
        let width = advance(*c, da, size);
        let x = area.x0() + (offset + i as f64) * cell + (cell - width) / 2.0;
        matrix(out, da, x, y)?;
        out.text(&format!("<{:02X}> Tj\n", c.code))?;
    }
    Ok(())
}

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

fn encode(font: &Font, layout: &TextLayout<'_>, limits: &Limits) -> Result<Vec<Character>> {
    let text = layout.text;
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
            input.next_if_eq(&'\n');
            '\n'
        } else {
            c
        };
        if c == '\n' && layout.multiline {
            chars.push(Character {
                code: 0,
                advance: 0.0,
                space: false,
                newline: true,
            });
        } else {
            let c = if layout.password {
                '*'
            } else if c == '\t' {
                ' '
            } else {
                c
            };
            let code = font.encode(c)?;
            chars.push(Character {
                code: code.code,
                advance: code.advance,
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

/// How lines are measured: at `size`, against `width`, wrapping or not.
struct Measure<'a> {
    da: &'a DefaultAppearance,
    size: f64,
    width: f64,
    wrap: bool,
}

/// Greedy lines: explicit newlines always break; with wrapping, a line
/// breaks at its last space, or before the character that overflows.
fn lines(
    chars: &[Character],
    measure: &Measure<'_>,
    limits: &Limits,
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
        let next = measured + advance(*c, measure.da, measure.size);
        if !next.is_finite() {
            return Err(Error::limit("appearance text width overflow"));
        }
        if measure.wrap && next > measure.width && i > start {
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

/// The inputs of choosing an automatic font size (DA size 0).
struct Sizing<'a> {
    chars: &'a [Character],
    font: &'a Font,
    da: &'a DefaultAppearance,
    area: Rect,
    layout: &'a TextLayout<'a>,
    limits: &'a Limits,
    work: &'a mut usize,
}

impl Sizing<'_> {
    /// The largest size up to 12 pt and the box height that fits, found by
    /// bisection and floored to 1/64 pt.
    fn auto_size(&mut self) -> Result<f64> {
        let cap = 12.0_f64
            .min(self.area.height() / ((self.font.ascent - self.font.descent) * self.da.matrix[3]));
        if self.fits(cap)? {
            return Ok(cap);
        }
        let (mut low, mut high) = (0.0, cap);
        for _ in 0..24 {
            let middle = f64::midpoint(low, high);
            if self.fits(middle)? {
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

    fn fits(&mut self, size: f64) -> Result<bool> {
        let (area, da) = (self.area, self.da);
        if let Some(cells) = self.layout.comb {
            if cells == 0 || self.chars.len() > cells {
                return Err(Error::invalid_argument("invalid Comb cell count"));
            }
            return Ok(self
                .chars
                .iter()
                .all(|c| advance(*c, da, size) <= area.width() / cells as f64));
        }
        let lines = lines(
            self.chars,
            &Measure {
                da,
                size,
                width: area.width(),
                wrap: self.layout.multiline,
            },
            self.limits,
            self.work,
        )?;
        Ok(lines.iter().all(|line| line.width <= area.width())
            && (self.font.ascent - self.font.descent) * size * da.matrix[3]
                + lines.len().saturating_sub(1) as f64 * leading(da, size) * da.matrix[3]
                <= area.height())
    }
}
