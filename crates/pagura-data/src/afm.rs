//! Bounded parser for the pinned Core14 AFM files, not a PostScript interpreter.
use pagura_core::{Error, Limits, Meter, Name, Position, Result, ResultExt};
use pagura_text::{StandardDescriptor, StandardMetrics};
use std::collections::BTreeMap;

/// The part of the file a line belongs to.
#[derive(Clone, Copy)]
enum Section {
    /// Global font information, before `StartCharMetrics` or after
    /// `EndCharMetrics`.
    Header,
    /// Inside `StartCharMetrics`, with this many metrics lines still due.
    CharMetrics { remaining: usize },
}

/// Metrics read so far. The font name is required and has no default.
struct Builder {
    name: Option<Name>,
    encoding: [Option<Name>; 256],
    widths: BTreeMap<Name, f64>,
    descriptor: StandardDescriptor,
}

/// Parses one AFM file. Every line is charged to `font_work` and every
/// glyph to `font_entries`; errors carry the offset of their line.
pub(crate) fn parse(data: &[u8], limits: &Limits) -> Result<StandardMetrics> {
    // A bundled file stands in for the decoded stream a PDF would embed.
    if data.len() > limits.decoded_bytes {
        return Err(Error::limit("AFM byte budget exceeded"));
    }
    let text = std::str::from_utf8(data).map_err(|error| {
        Error::syntax("AFM is not UTF-8").at(Position::Buffer(error.valid_up_to()))
    })?;
    if !text.starts_with("StartFontMetrics ") || !text.trim_end().ends_with("EndFontMetrics") {
        return Err(Error::syntax("invalid AFM framing"));
    }
    let mut work = Meter::new(limits.font_work, "AFM work budget exceeded");
    let mut glyphs = Meter::new(limits.font_entries, "AFM glyph budget exceeded");
    let mut metrics = Builder {
        name: None,
        encoding: std::array::from_fn(|_| None),
        widths: BTreeMap::new(),
        descriptor: StandardDescriptor::default(),
    };
    let mut section = Section::Header;
    for (offset, line) in crate::lines(text) {
        section = work
            .charge(1)
            .and_then(|()| metrics.line(line.trim(), section, limits, &mut glyphs))
            .at(Position::Buffer(offset))?;
    }
    match (section, metrics.name) {
        (Section::Header, Some(name)) if !metrics.widths.is_empty() => Ok(StandardMetrics {
            name,
            encoding: metrics.encoding,
            widths: metrics.widths,
            descriptor: metrics.descriptor,
        }),
        _ => Err(Error::syntax("incomplete AFM metrics")),
    }
}

impl Builder {
    /// Reads one trimmed line and returns the section of the next line.
    fn line(
        &mut self,
        line: &str,
        section: Section,
        limits: &Limits,
        glyphs: &mut Meter,
    ) -> Result<Section> {
        let (key, value) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let value = value.trim();
        match (section, key) {
            (Section::CharMetrics { remaining: 0 }, "EndCharMetrics") => Ok(Section::Header),
            (
                Section::CharMetrics {
                    remaining: remaining @ 1..,
                },
                _,
            ) if key != "EndCharMetrics" => {
                glyphs.charge(1)?;
                self.character(line, limits)?;
                Ok(Section::CharMetrics {
                    remaining: remaining - 1,
                })
            }
            (Section::CharMetrics { .. }, _) | (Section::Header, "EndCharMetrics") => {
                Err(Error::syntax("AFM character count mismatch"))
            }
            (Section::Header, "StartCharMetrics") => Ok(Section::CharMetrics {
                remaining: value
                    .parse()
                    .map_err(|_| Error::syntax("invalid AFM character count"))?,
            }),
            (Section::Header, _) => {
                self.descriptor(key, value, limits)?;
                Ok(Section::Header)
            }
        }
    }

    fn descriptor(&mut self, key: &str, value: &str, limits: &Limits) -> Result<()> {
        let d = &mut self.descriptor;
        match key {
            "FontName" => self.name = Some(name(value, limits)?),
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
            "Weight" => bounded(value, limits)?.clone_into(&mut d.weight),
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

    /// Reads one `C code ; WX width ; N name ; …` metrics line.
    fn character(&mut self, line: &str, limits: &Limits) -> Result<()> {
        let mut code = None;
        let mut width = None;
        let mut glyph = None;
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
                    glyph = Some(name(
                        parts
                            .next()
                            .ok_or_else(|| Error::syntax("missing AFM name"))?,
                        limits,
                    )?);
                }
                _ => {}
            }
        }
        // -1 marks a glyph outside the built-in encoding.
        let code = code
            .filter(|code| (-1..=255).contains(code))
            .ok_or_else(|| Error::syntax("invalid AFM code"))?;
        let glyph = glyph.ok_or_else(|| Error::syntax("missing AFM name"))?;
        let width = width.ok_or_else(|| Error::syntax("missing AFM width"))?;
        if let Ok(code) = u8::try_from(code)
            && self.encoding[usize::from(code)]
                .replace(glyph.clone())
                .is_some()
        {
            return Err(Error::syntax("duplicate AFM code"));
        }
        if self.widths.insert(glyph, width).is_some() {
            return Err(Error::syntax("duplicate AFM glyph name"));
        }
        Ok(())
    }
}

fn number(value: &str) -> Result<f64> {
    value
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| Error::syntax("invalid AFM number"))
}

/// A non-empty value within the `string_bytes` budget.
fn bounded<'a>(value: &'a str, limits: &Limits) -> Result<&'a str> {
    if value.is_empty() {
        return Err(Error::syntax("empty AFM name"));
    }
    if value.len() > limits.string_bytes {
        return Err(Error::limit("AFM name budget exceeded"));
    }
    Ok(value)
}

fn name(value: &str, limits: &Limits) -> Result<Name> {
    Name::try_new(bounded(value, limits)?.as_bytes()).map_err(|_| Error::syntax("invalid AFM name"))
}

#[cfg(test)]
mod tests {
    use super::parse;
    use pagura_core::{ErrorKind, Limits, Position, Result};

    fn afm(body: &str) -> String {
        format!(
            "StartFontMetrics 4.1\nFontName Example\nStartCharMetrics 1\n{body}\nEndCharMetrics\nEndFontMetrics\n"
        )
    }

    #[test]
    fn nonfinite_metrics_and_mismatched_counts_are_syntax_errors() {
        for data in [
            afm("C 65 ; WX NaN ; N A ;"),
            afm("C 256 ; WX 600 ; N A ;"),
            afm("C 65 ; WX 600 ;"),
            afm(""),
            afm("C 65 ; WX 600 ; N A ;\nC 66 ; WX 600 ; N B ;"),
            afm("C 65 ; WX 600 ; N A\0 ;"),
            "StartFontMetrics 4.1\nFontName Example\nEndCharMetrics\nEndFontMetrics\n".to_owned(),
            "StartFontMetrics 4.1\nFontName\nStartCharMetrics 1\nC 65 ; WX 600 ; N A ;\nEndCharMetrics\nEndFontMetrics\n".to_owned(),
            afm("C 65 ; WX 600 ; N A ;").replace("FontName Example\n", ""),
        ] {
            let error = parse(data.as_bytes(), &Limits::default()).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Syntax, "{data:?}");
        }
    }

    #[test]
    fn metrics_are_read_and_errors_name_their_line() -> Result<()> {
        let metrics = parse(afm("C 65 ; WX 600 ; N A ;").as_bytes(), &Limits::default())?;
        assert_eq!(metrics.name.as_bytes(), b"Example");
        assert_eq!(metrics.width(65), Some(600.0));
        let data = afm("C 65 ; WX NaN ; N A ;");
        let error = parse(data.as_bytes(), &Limits::default()).unwrap_err();
        let line = data.find("C 65").unwrap_or_default();
        assert_eq!(error.position, Position::Buffer(line));
        Ok(())
    }

    #[test]
    fn lines_are_font_work_and_glyphs_are_font_entries() {
        let data = afm("C 65 ; WX 600 ; N A ;");
        for (limits, message) in [
            (
                Limits {
                    font_work: 3,
                    ..Limits::default()
                },
                "AFM work budget exceeded",
            ),
            (
                Limits {
                    font_entries: 0,
                    ..Limits::default()
                },
                "AFM glyph budget exceeded",
            ),
            (
                Limits {
                    string_bytes: 6,
                    ..Limits::default()
                },
                "AFM name budget exceeded",
            ),
            (
                Limits {
                    decoded_bytes: data.len() - 1,
                    ..Limits::default()
                },
                "AFM byte budget exceeded",
            ),
        ] {
            let error = parse(data.as_bytes(), &limits).unwrap_err();
            assert_eq!((error.kind, error.message), (ErrorKind::Limit, message));
        }
        // Content-stream operations and object values are not font budgets
        // (ADR 0016).
        let limits = Limits {
            operations: 0,
            value_nodes: 0,
            ..Limits::default()
        };
        assert!(parse(data.as_bytes(), &limits).is_ok());
    }
}
