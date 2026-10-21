use super::*;

fn n(value: f64) -> Value {
    Value::Real(value)
}

#[test]
fn every_operator_round_trips_through_its_operation() {
    let number = Value::Integer(0);
    for operator in Operator::ALL {
        let operands = match operator.as_bytes() {
            b"BDC" | b"DP" => vec![Value::name(b"T"), Value::name(b"P")],
            b"BMC" | b"CS" | b"cs" | b"Do" | b"gs" | b"MP" | b"ri" | b"sh" => {
                vec![Value::name(b"N")]
            }
            b"c" | b"cm" | b"d1" | b"Tm" => vec![number.clone(); 6],
            b"d" => vec![Value::Array(vec![]), number.clone()],
            b"d0" | b"l" | b"m" | b"Td" | b"TD" => vec![number.clone(); 2],
            b"G" | b"g" | b"i" | b"j" | b"J" | b"M" | b"Tc" | b"TL" | b"Tr" | b"Ts" | b"Tw"
            | b"Tz" | b"w" => vec![number.clone()],
            b"K" | b"k" | b"re" | b"v" | b"y" => vec![number.clone(); 4],
            b"RG" | b"rg" => vec![number.clone(); 3],
            b"Tf" => vec![Value::name(b"F"), number.clone()],
            b"Tj" | b"'" => vec![Value::String(vec![])],
            b"TJ" => vec![Value::Array(vec![])],
            b"\"" => vec![number.clone(), number.clone(), Value::String(vec![])],
            _ => vec![],
        };
        match Op::parse(operator, operands) {
            Ok(op) => assert_eq!(op.operator(), operator, "{operator:?}"),
            Err(error) => assert!(
                matches!(operator.as_bytes(), b"BI" | b"ID" | b"EI"),
                "{operator:?}: {error}"
            ),
        }
    }
}

#[test]
fn counts_are_checked_before_types() {
    let error = Op::parse(Operator::MoveTo, vec![Value::name(b"x")]).unwrap_err();
    assert_eq!(error.message, "wrong operand count");
    let error = Op::parse(Operator::MoveTo, vec![Value::name(b"x"), n(1.0)]).unwrap_err();
    assert_eq!(error.message, "wrong operand type");
    let error = Op::parse(Operator::LineCap, vec![n(1.0)]).unwrap_err();
    assert_eq!(error.message, "wrong operand type");
    let error = Op::parse(Operator::EndInlineImage, vec![]).unwrap_err();
    assert_eq!(error.message, "inline image delimiter outside image");
}

#[test]
fn colour_operands_allow_a_final_pattern_name_only_with_scn() {
    let operands = vec![n(0.5), Value::name(b"P0")];
    assert_eq!(
        Op::parse(Operator::FillColorN, operands.clone()).unwrap(),
        Op::FillColorN(ColorOperands {
            components: vec![0.5],
            pattern: Some(Name::new(b"P0")),
        })
    );
    assert!(Op::parse(Operator::FillColor, operands).is_err());
    assert!(Op::parse(Operator::FillColorN, vec![Value::name(b"P0"), n(0.5)]).is_err());
    let op = Op::parse(Operator::StrokeColor, vec![]).unwrap();
    assert!(matches!(op, Op::StrokeColor(c) if c.is_empty()));
}

#[test]
fn dash_arrays_and_text_arrays_are_validated() {
    let dash = |values: Vec<Value>| {
        Op::parse(Operator::Dash, vec![Value::Array(values), Value::Integer(2)])
    };
    assert_eq!(
        dash(vec![Value::Integer(3), n(1.5)]).unwrap(),
        Op::Dash(Dash {
            lengths: vec![3.0, 1.5],
            phase: 2.0
        })
    );
    assert_eq!(
        dash(vec![Value::Integer(0)]).unwrap_err().message,
        "dash array is all zero"
    );
    assert_eq!(
        dash(vec![n(-1.0)]).unwrap_err().message,
        "invalid dash length"
    );
    let tj = Op::parse(
        Operator::ShowTextAdjusted,
        vec![Value::Array(vec![
            Value::String(b"A".to_vec()),
            Value::Integer(-120),
        ])],
    )
    .unwrap();
    assert_eq!(
        tj,
        Op::ShowTextAdjusted(vec![
            TextItem::Show(b"A".to_vec()),
            TextItem::Adjust(-120.0)
        ])
    );
    let error = Op::parse(
        Operator::ShowTextAdjusted,
        vec![Value::Array(vec![Value::name(b"x")])],
    )
    .unwrap_err();
    assert_eq!(error.message, "TJ array requires numbers or strings");
}

#[test]
fn painting_operators_know_their_rules() {
    assert_eq!(Painting::FillStrokeEvenOdd.fill(), Some(Rule::EvenOdd));
    assert_eq!(Painting::FillObsolete.fill(), Some(Rule::NonZero));
    assert!(Painting::CloseFillStroke.closes() && Painting::CloseFillStroke.strokes());
    assert!(!Painting::EndPath.strokes() && Painting::EndPath.fill().is_none());
    assert!(Op::NextLine.is_text() && !Op::BeginText.is_text());
}
