//! Annex A recognition and operand validation, independent of implementation.

use pagura_core::{Error, Feature, Result, Value};

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

pub(crate) fn validate(operator: &[u8], values: &[Value]) -> Result<Option<Feature>> {
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
        return Ok(Some(crate::features::operator_tag(operator)));
    }
    if matches!(operator, b"SC" | b"sc" | b"SCN" | b"scn") {
        let allow_name = matches!(operator, b"SCN" | b"scn");
        if values.iter().enumerate().any(|(i, v)| {
            v.as_number().is_none()
                && !(allow_name && i + 1 == values.len() && v.as_name().is_some())
        }) {
            return Err(Error::syntax("wrong colour operand type"));
        }
        return Ok(Some(crate::features::operator_tag(operator)));
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
