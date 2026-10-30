//! Strict, bounded default appearance parsing (12.7.4.3; 9.3).

use crate::features;
use pagura_core::{
    Error, Limits, Name, Result, Value,
    syntax::{Origin, Parser},
};

/// A field's declared font name and effective horizontal text size, as
/// native editors show them.
#[derive(Debug, Clone, PartialEq)]
pub struct TextStyle {
    font: Name,
    size: f64,
}

impl TextStyle {
    /// The DA font's resource name.
    pub fn font(&self) -> &Name {
        &self.font
    }

    /// The DA size scaled by the DA text matrix's vertical scale; 0 means
    /// automatic size.
    pub fn size(&self) -> f64 {
        self.size
    }
}

/// Reads the font name and effective text size of a default appearance
/// string.
///
/// # Errors
/// As default-appearance parsing: syntax and limit errors, and unsupported
/// transformations and operators.
pub fn default_text_style(bytes: &[u8], limits: &Limits) -> Result<TextStyle> {
    let appearance = DefaultAppearance::parse(bytes, limits)?;
    Ok(TextStyle {
        size: appearance.size * appearance.matrix[3],
        font: appearance.font,
    })
}

/// A parsed DA string: the text parameters layout uses, and the bytes of
/// every other operator, replayed verbatim before the text.
#[derive(Debug, Clone)]
pub(crate) struct DefaultAppearance {
    pub(crate) font: Name,
    pub(crate) size: f64,
    pub(crate) state: Vec<u8>,
    pub(crate) matrix: [f64; 4],
    pub(crate) character_space: f64,
    pub(crate) word_space: f64,
    pub(crate) scale: f64,
    pub(crate) leading: f64,
    pub(crate) rise: f64,
}

/// Which of the once-only operators a DA string has shown.
#[derive(Default)]
struct Seen {
    font: bool,
    matrix: bool,
}

impl DefaultAppearance {
    pub(crate) fn parse(bytes: &[u8], limits: &Limits) -> Result<Self> {
        if bytes.len() > limits.string_bytes {
            return Err(Error::limit("default appearance string budget exceeded"));
        }
        let mut parser = Parser::new(bytes, Origin::Buffer, *limits);
        let mut result = Self {
            font: Name::new(b""),
            size: 0.0,
            state: Vec::new(),
            matrix: [1.0, 0.0, 0.0, 1.0],
            character_space: 0.0,
            word_space: 0.0,
            scale: 1.0,
            leading: 0.0,
            rise: 0.0,
        };
        let mut operands = Vec::new();
        let mut start = 0;
        let mut seen = Seen::default();
        let mut tokens = 0usize;
        while let Some(next) = parser.peek() {
            if tokens >= limits.operations {
                return Err(
                    Error::limit("default appearance token budget exceeded").at(parser.position())
                );
            }
            tokens += 1;
            if operands.is_empty() {
                start = parser.offset();
            }
            if matches!(next, b'/' | b'+' | b'-' | b'.' | b'0'..=b'9' | b'[') {
                if operands.len() >= 8 {
                    return Err(
                        Error::syntax("excess default appearance operands").at(parser.position())
                    );
                }
                operands.push(parser.parse()?);
                continue;
            }
            let position = parser.position();
            let op = parser.word()?;
            let special = result
                .apply(op, &operands, &mut seen)
                .map_err(|e| e.at(position))?;
            if !special {
                let operation = bytes
                    .get(start..parser.offset())
                    .ok_or(Error::syntax("invalid default appearance operator"))?;
                result.state.extend_from_slice(operation);
                result.state.push(b'\n');
            }
            operands.clear();
        }
        if !seen.font || !operands.is_empty() {
            return Err(
                Error::syntax("DA requires Tf and complete operator operands")
                    .at(parser.position()),
            );
        }
        if result.scale <= 0.0 || result.leading < 0.0 {
            return Err(Error::unsupported(
                features::APPEARANCE_TEXT_MATRIX,
                "nonpositive text scale or negative leading is unavailable",
            ));
        }
        Ok(result)
    }

    /// Applies one operator; true for Tf and Tm, which layout writes itself.
    fn apply(&mut self, op: &[u8], operands: &[Value], seen: &mut Seen) -> Result<bool> {
        let special = match op {
            b"Tf" => {
                self.font(operands)?;
                seen.font = true;
                true
            }
            b"Tm" => {
                let values = numbers(operands, 6)?;
                if seen.matrix {
                    return Err(Error::syntax("DA may contain at most one Tm"));
                }
                let [a, b, c, d, ..] = values.as_slice() else {
                    return Err(Error::syntax("wrong DA operand count"));
                };
                self.matrix = [*a, *b, *c, *d];
                if *a <= 0.0 || *d <= 0.0 || *b != 0.0 || *c != 0.0 {
                    return Err(Error::unsupported(
                        features::APPEARANCE_TEXT_MATRIX,
                        "oblique or reflected variable text matrices are unavailable",
                    ));
                }
                seen.matrix = true;
                true
            }
            b"Tc" | b"Tw" | b"Tz" | b"TL" | b"Ts" => {
                let value = number(operands)?;
                match op {
                    b"Tc" => self.character_space = value,
                    b"Tw" => self.word_space = value,
                    b"Tz" => self.scale = value / 100.0,
                    b"TL" => self.leading = value,
                    _ => self.rise = value,
                }
                false
            }
            b"Tr" => {
                if !matches!(operands, [Value::Integer(0..=7)]) {
                    return Err(Error::syntax("invalid DA rendering mode"));
                }
                false
            }
            b"g" | b"G" | b"w" | b"M" | b"i" => {
                numbers(operands, 1)?;
                false
            }
            b"rg" | b"RG" => {
                numbers(operands, 3)?;
                false
            }
            b"k" | b"K" => {
                numbers(operands, 4)?;
                false
            }
            b"J" | b"j" => {
                if !matches!(operands, [Value::Integer(0..=2)]) {
                    return Err(Error::syntax("invalid DA line cap or join"));
                }
                false
            }
            b"ri" => {
                if !matches!(operands, [Value::Name(_)]) {
                    return Err(Error::syntax("DA ri requires a name"));
                }
                false
            }
            _ => {
                return Err(Error::unsupported(
                    features::APPEARANCE_DEFAULT_APPEARANCE,
                    "default appearance operator is unavailable",
                ));
            }
        };
        Ok(special)
    }

    /// `font size Tf`; a negative size is valid PDF but unsupported here.
    fn font(&mut self, operands: &[Value]) -> Result<()> {
        let [name, size] = operands else {
            return Err(Error::syntax("DA Tf requires a font and size"));
        };
        self.font = name
            .as_name()
            .ok_or_else(|| Error::syntax("DA Tf font must be a name"))?
            .clone();
        self.size = numeric(size)?;
        if self.size < 0.0 {
            return Err(Error::unsupported(
                features::APPEARANCE_TEXT_MATRIX,
                "negative variable text font sizes are unavailable",
            ));
        }
        Ok(())
    }
}

fn numeric(value: &Value) -> Result<f64> {
    value
        .as_number()
        .filter(|n| n.is_finite())
        .ok_or(Error::syntax("DA operand must be finite numeric"))
}

fn numbers(values: &[Value], count: usize) -> Result<Vec<f64>> {
    if values.len() != count {
        return Err(Error::syntax("wrong DA operand count"));
    }
    values.iter().map(numeric).collect()
}

fn number(values: &[Value]) -> Result<f64> {
    match values {
        [value] => numeric(value),
        _ => Err(Error::syntax("wrong DA operand count")),
    }
}
