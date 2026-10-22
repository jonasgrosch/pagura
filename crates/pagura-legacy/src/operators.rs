//! Annex A recognition and operand validation for implemented content operators.

use pagura_core::{Error, Result, Value};

/// Recognition of an implemented Annex A operator, after operand validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Implemented;

#[derive(Clone, Copy)]
enum Kind {
    Number,
    Integer,
    Name,
    String,
    Array,
    Properties,
}

impl Kind {
    fn accepts(self, value: &Value) -> bool {
        match self {
            Self::Number => value.as_number().is_some(),
            Self::Integer => value.as_integer().is_some(),
            Self::Name => value.as_name().is_some(),
            Self::String => value.as_string().is_some(),
            Self::Array => value.as_array().is_some(),
            Self::Properties => value.as_name().is_some() || value.as_dictionary().is_some(),
        }
    }
}

pub(crate) fn validate(operator: &[u8], values: &[Value]) -> Result<Option<Implemented>> {
    use Kind::{Array as A, Integer as I, Name as M, Number as N, Properties as P, String as S};
    macro_rules! table {
        ($($name:literal => [$($kind:expr),*]),* $(,)?) => {
            match operator {
                $( $name => { check(values, &[$($kind),*])?; Some(()) } ,)*
                _ => None,
            }
        }
    }
    let known = table! {
        b"b" => [], b"B" => [], b"b*" => [], b"B*" => [],
        b"BDC" => [M,P], b"BI" => [], b"BMC" => [M], b"BT" => [], b"BX" => [],
        b"c" => [N,N,N,N,N,N], b"cm" => [N,N,N,N,N,N], b"CS" => [M], b"cs" => [M],
        b"d" => [A,N], b"d0" => [N,N], b"d1" => [N,N,N,N,N,N], b"Do" => [M], b"DP" => [M,P],
        b"EI" => [], b"EMC" => [], b"ET" => [], b"EX" => [],
        b"f" => [], b"F" => [], b"f*" => [], b"G" => [N], b"g" => [N], b"gs" => [M],
        b"h" => [], b"i" => [N], b"ID" => [], b"j" => [I], b"J" => [I],
        b"K" => [N,N,N,N], b"k" => [N,N,N,N], b"l" => [N,N], b"m" => [N,N], b"M" => [N], b"MP" => [M],
        b"n" => [], b"q" => [], b"Q" => [], b"re" => [N,N,N,N], b"RG" => [N,N,N], b"rg" => [N,N,N],
        b"ri" => [M], b"s" => [], b"S" => [], b"sh" => [M],
        b"T*" => [], b"Tc" => [N], b"Td" => [N,N], b"TD" => [N,N], b"Tf" => [M,N], b"Tj" => [S], b"TJ" => [A],
        b"TL" => [N], b"Tm" => [N,N,N,N,N,N], b"Tr" => [I], b"Ts" => [N], b"Tw" => [N], b"Tz" => [N],
        b"v" => [N,N,N,N], b"w" => [N], b"W" => [], b"W*" => [], b"y" => [N,N,N,N], b"'" => [S], b"\"" => [N,N,S]
    };
    if known.is_some() {
        if operator == b"TJ"
            && values
                .first()
                .and_then(Value::as_array)
                .is_some_and(|array| {
                    array
                        .iter()
                        .any(|v| v.as_number().is_none() && v.as_string().is_none())
                })
        {
            return Err(Error::syntax("TJ array requires numbers or strings"));
        }
        return Ok(Some(Implemented));
    }
    if matches!(operator, b"SC" | b"sc" | b"SCN" | b"scn") {
        let allow_name = matches!(operator, b"SCN" | b"scn");
        if values.iter().enumerate().any(|(i, v)| {
            v.as_number().is_none()
                && !(allow_name && i + 1 == values.len() && v.as_name().is_some())
        }) {
            return Err(Error::syntax("wrong colour operand type"));
        }
        return Ok(Some(Implemented));
    }
    Ok(None)
}

fn check(values: &[Value], kinds: &[Kind]) -> Result<()> {
    if values.len() != kinds.len() {
        return Err(Error::syntax("wrong operand count"));
    }
    if !values
        .iter()
        .zip(kinds)
        .all(|(value, &kind)| kind.accepts(value))
    {
        return Err(Error::syntax("wrong operand type"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_text_and_image_operator_validates_as_implemented() {
        let number = Value::Integer(0);
        for (operator, operands) in [
            (b"BT".as_slice(), vec![]),
            (b"ET", vec![]),
            (b"Tc", vec![number.clone()]),
            (b"Tw", vec![number.clone()]),
            (b"Tz", vec![number.clone()]),
            (b"TL", vec![number.clone()]),
            (b"Ts", vec![number.clone()]),
            (b"Tr", vec![number.clone()]),
            (b"Tf", vec![Value::name(b"F"), number.clone()]),
            (b"Tm", vec![number.clone(); 6]),
            (b"Td", vec![number.clone(); 2]),
            (b"TD", vec![number.clone(); 2]),
            (b"T*", vec![]),
            (b"Tj", vec![Value::String(vec![])]),
            (b"TJ", vec![Value::Array(vec![])]),
            (b"'", vec![Value::String(vec![])]),
            (
                b"\"",
                vec![number.clone(), number.clone(), Value::String(vec![])],
            ),
            (b"d0", vec![number.clone(); 2]),
            (b"d1", vec![number; 6]),
            (b"Do", vec![Value::name(b"I")]),
            (b"BI", vec![]),
            (b"ID", vec![]),
            (b"EI", vec![]),
        ] {
            assert_eq!(
                validate(operator, &operands).unwrap(),
                Some(Implemented),
                "{operator:?}"
            );
        }
    }
}
