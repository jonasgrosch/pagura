//! Strict, bounded default appearance parsing (12.7.4.3; 9.3).
use crate::operators::{MAX_OPERANDS, Token, token};
use pagura_core::{
    Error, Limits, Name, Result, Value,
    syntax::{Origin, Parser},
};

#[derive(Debug, Clone)]
pub(super) struct DefaultAppearance {
    pub font: Name,
    pub size: f64,
    pub state: Vec<u8>,
    pub matrix: [f64; 4],
    pub character_space: f64,
    pub word_space: f64,
    pub scale: f64,
    pub leading: f64,
    pub rise: f64,
}
impl DefaultAppearance {
    pub(super) fn parse(bytes: &[u8], limits: Limits) -> Result<Self> {
        if bytes.len() > limits.string_bytes {
            return Err(Error::limit("default appearance string budget exceeded"));
        }
        let mut parser = Parser::new(bytes, Origin::Buffer, limits);
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
        let mut font = false;
        let mut matrix = false;
        let mut tokens = 0usize;
        while parser.peek().is_some() {
            if tokens >= limits.operations {
                return Err(
                    Error::limit("default appearance token budget exceeded").at(parser.position())
                );
            }
            tokens += 1;
            if operands.is_empty() {
                start = parser.offset();
            }
            let position = parser.position();
            let op = match token(&mut parser)? {
                Some(Token::Operand(value)) => {
                    if operands.len() >= MAX_OPERANDS {
                        return Err(Error::limit("content operand budget exceeded").at(position));
                    }
                    operands.push(value);
                    continue;
                }
                Some(Token::Operator(op)) => op,
                None => break,
            };
            let special = result
                .apply(op, &operands, &mut font, &mut matrix)
                .map_err(|e| e.at(position))?;
            if !special {
                result
                    .state
                    .extend_from_slice(&bytes[start..parser.offset()]);
                result.state.push(b'\n');
            }
            operands.clear();
        }
        if !font || !operands.is_empty() {
            return Err(
                Error::syntax("DA requires Tf and complete operator operands")
                    .at(parser.position()),
            );
        }
        if result.scale <= 0.0 || result.leading < 0.0 {
            return Err(Error::unsupported(
                crate::features::APPEARANCE_TEXT_MATRIX,
                "nonpositive text scale or negative leading is unavailable",
            ));
        }
        Ok(result)
    }
    fn apply(
        &mut self,
        op: &[u8],
        operands: &[Value],
        font: &mut bool,
        matrix: &mut bool,
    ) -> Result<bool> {
        let special = match op {
            b"Tf" => {
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
                        crate::features::APPEARANCE_TEXT_MATRIX,
                        "negative variable text font sizes are unavailable",
                    ));
                }
                *font = true;
                true
            }
            b"Tm" => {
                let values = numbers(operands, 6)?;
                if *matrix {
                    return Err(Error::syntax("DA may contain at most one Tm"));
                }
                self.matrix.copy_from_slice(&values[..4]);
                if values[0] <= 0.0 || values[3] <= 0.0 || values[1] != 0.0 || values[2] != 0.0 {
                    return Err(Error::unsupported(
                        crate::features::APPEARANCE_TEXT_MATRIX,
                        "oblique or reflected variable text matrices are unavailable",
                    ));
                }
                *matrix = true;
                true
            }
            b"Tc" | b"Tw" | b"Tz" | b"TL" | b"Ts" => {
                let values = numbers(operands, 1)?;
                match op {
                    b"Tc" => self.character_space = values[0],
                    b"Tw" => self.word_space = values[0],
                    b"Tz" => self.scale = values[0] / 100.0,
                    b"TL" => self.leading = values[0],
                    _ => self.rise = values[0],
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
                    crate::features::APPEARANCE_DEFAULT_APPEARANCE,
                    "default appearance operator is unavailable",
                ));
            }
        };
        Ok(special)
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
