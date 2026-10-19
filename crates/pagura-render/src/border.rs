//! Annotation border width and style (12.5.4, Tables 166 and 168), read
//! once for annotation appearances and generated widget appearances.

use pagura_core::{Dictionary, Error, Result, Value};
use pagura_document::Document;

/// Table 168 `S`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BorderStyle {
    Solid,
    /// `D` with its dash array, `[3]` by default; also the dash array of a
    /// legacy `Border` array.
    Dashed(Vec<f64>),
    Beveled,
    Inset,
    Underline,
    /// A style name Table 168 does not define.
    Other,
}

/// A border from `BS`, else from the legacy `Border` array, else a solid
/// one-unit border.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Border {
    pub(crate) width: f64,
    pub(crate) style: BorderStyle,
    /// The legacy `Border` array gives nonzero corner radii.
    pub(crate) rounded: bool,
}

impl Border {
    pub(crate) fn read(document: &Document, d: &Dictionary) -> Result<Self> {
        let mut border = Self {
            width: 1.0,
            style: BorderStyle::Solid,
            rounded: false,
        };
        if let Some(bs) = entry(document, d, b"BS")? {
            let bs = bs
                .as_dictionary()
                .ok_or(Error::syntax("BS must be a dictionary"))?;
            if let Some(width) = entry(document, bs, b"W")? {
                border.width = number(document, width)?;
            }
            if let Some(style) = entry(document, bs, b"S")? {
                let style = style
                    .as_name()
                    .ok_or(Error::syntax("BS S must be a name"))?;
                border.style = match style.as_bytes() {
                    b"S" => BorderStyle::Solid,
                    b"D" => BorderStyle::Dashed(match entry(document, bs, b"D")? {
                        Some(dash) => crate::numbers::list(document, dash, f64::MAX)?,
                        None => vec![3.0],
                    }),
                    b"B" => BorderStyle::Beveled,
                    b"I" => BorderStyle::Inset,
                    b"U" => BorderStyle::Underline,
                    _ => BorderStyle::Other,
                };
            }
        } else if let Some(array) = entry(document, d, b"Border")? {
            let array = array
                .as_array()
                .ok_or(Error::syntax("Border must be an array"))?;
            let [horizontal, vertical, width, dash @ ..] = array else {
                return Err(Error::syntax("Border requires three or four entries"));
            };
            if dash.len() > 1 {
                return Err(Error::syntax("Border requires three or four entries"));
            }
            border.rounded =
                number(document, horizontal)? != 0.0 || number(document, vertical)? != 0.0;
            border.width = number(document, width)?;
            if let Some(dash) = dash.first() {
                border.style = BorderStyle::Dashed(crate::numbers::list(document, dash, f64::MAX)?);
            }
        }
        if border.width < 0.0 {
            return Err(Error::syntax("negative border width"));
        }
        if let BorderStyle::Dashed(dash) = &border.style
            && (dash.iter().any(|v| *v < 0.0) || !dash.is_empty() && dash.iter().all(|v| *v == 0.0))
        {
            return Err(Error::syntax("invalid border dash pattern"));
        }
        Ok(border)
    }
}

/// The resolved entry `key`, treating null as absent (7.3.9).
fn entry<'a>(document: &'a Document, d: &'a Dictionary, key: &[u8]) -> Result<Option<&'a Value>> {
    Ok(d.get(key)
        .map(|v| document.resolve(v))
        .transpose()?
        .filter(|v| !v.is_null()))
}

fn number(document: &Document, value: &Value) -> Result<f64> {
    crate::numbers::number(document, value, f64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagura_core::syntax::{Origin, Parser};
    use pagura_core::{ErrorKind, Limits};
    use pagura_document::OpenOptions;

    fn read(source: &[u8]) -> Result<Border> {
        // Direct values only; any document resolves them.
        let document = Document::open(
            pagura_testkit::fixture("content", "compound-path.pdf"),
            &OpenOptions::default(),
        )?;
        let value = Parser::new(source, Origin::Buffer, Limits::default()).parse()?;
        Border::read(&document, value.as_dictionary().unwrap())
    }

    #[test]
    fn border_styles_follow_tables_166_and_168() {
        let border = |width, style, rounded| Border {
            width,
            style,
            rounded,
        };
        for (source, expected) in [
            (b"<< >>".as_slice(), border(1.0, BorderStyle::Solid, false)),
            (
                b"<< /BS << /S /D >> >>",
                border(1.0, BorderStyle::Dashed(vec![3.0]), false),
            ),
            (
                b"<< /BS << /W 2 /S /U >> /Border [0 0 5] >>",
                border(2.0, BorderStyle::Underline, false),
            ),
            (
                b"<< /BS << /S /B >> >>",
                border(1.0, BorderStyle::Beveled, false),
            ),
            (
                b"<< /BS << /S /I >> >>",
                border(1.0, BorderStyle::Inset, false),
            ),
            (
                b"<< /BS << /S /X >> >>",
                border(1.0, BorderStyle::Other, false),
            ),
            (
                b"<< /BS null /Border [0 0 0] >>",
                border(0.0, BorderStyle::Solid, false),
            ),
            (
                b"<< /Border [0 0 2 [1 1]] >>",
                border(2.0, BorderStyle::Dashed(vec![1.0, 1.0]), false),
            ),
            (
                b"<< /Border [1 0 1] >>",
                border(1.0, BorderStyle::Solid, true),
            ),
        ] {
            assert_eq!(
                read(source).unwrap(),
                expected,
                "{}",
                String::from_utf8_lossy(source)
            );
        }
        for source in [
            b"<< /BS << /S 1 >> >>".as_slice(),
            b"<< /BS << /W -1 >> >>",
            b"<< /BS << /S /D /D [0 0] >> >>",
            b"<< /Border [0 0 1 [-1]] >>",
            b"<< /Border [0 0] >>",
            b"<< /Border [0 0 1 [1] 2] >>",
            b"<< /Border [/R 0 1] >>",
        ] {
            assert_eq!(
                read(source).unwrap_err().kind,
                ErrorKind::Syntax,
                "{}",
                String::from_utf8_lossy(source)
            );
        }
    }
}
