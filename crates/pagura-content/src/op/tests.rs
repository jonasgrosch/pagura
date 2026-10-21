use super::*;
use crate::{Content, Reader};
use pagura_core::{Limits, Meter};

/// Reads the one operation of `content` and hands it to `check`.
fn with_op<T>(content: &[u8], check: impl FnOnce(&Op<'_>) -> T) -> Result<T> {
    let content = Content::from_bytes(content);
    let mut reader = Reader::new(&content, &Limits::default());
    let mut tokens = Meter::new(1_000, "test budget exceeded");
    let operation = reader.next(&mut tokens)?.ok_or(Error::syntax("no operation"))?;
    Ok(check(&operation.op))
}

/// The operator of the one operation of `content`.
fn first_operator(content: &[u8]) -> Result<Operator> {
    let content = Content::from_bytes(content);
    let mut reader = Reader::new(&content, &Limits::default());
    let mut tokens = Meter::new(1_000, "test budget exceeded");
    let operation = reader.next(&mut tokens)?.ok_or(Error::syntax("no operation"))?;
    Ok(operation.op.operator())
}

fn error(content: &[u8]) -> &'static str {
    with_op(content, |_| ()).unwrap_err().message
}

#[test]
fn every_operator_round_trips_through_its_operation() {
    for operator in Operator::ALL {
        let operands = match operator.as_bytes() {
            b"BDC" | b"DP" => "/T /P",
            b"BMC" | b"CS" | b"cs" | b"Do" | b"gs" | b"MP" | b"ri" | b"sh" => "/N",
            b"c" | b"cm" | b"d1" | b"Tm" => "0 0 0 0 0 0",
            b"d" => "[] 0",
            b"d0" | b"l" | b"m" | b"Td" | b"TD" => "0 0",
            b"G" | b"g" | b"i" | b"j" | b"J" | b"M" | b"Tc" | b"TL" | b"Tr" | b"Ts" | b"Tw"
            | b"Tz" | b"w" => "0",
            b"K" | b"k" | b"re" | b"v" | b"y" => "0 0 0 0",
            b"RG" | b"rg" => "0 0 0",
            b"Tf" => "/F 0",
            b"Tj" | b"'" => "()",
            b"TJ" => "[]",
            b"\"" => "0 0 ()",
            _ => "",
        };
        let spelled = std::str::from_utf8(operator.as_bytes()).unwrap();
        let content = format!("{operands} {spelled} ");
        match first_operator(content.as_bytes()) {
            Ok(read) => assert_eq!(read, operator, "{operator:?}"),
            Err(error) => assert!(
                matches!(operator.as_bytes(), b"ID" | b"EI" | b"BI" | b"EX"),
                "{operator:?}: {error}"
            ),
        }
    }
}

#[test]
fn counts_are_checked_before_types() {
    assert_eq!(error(b"/x m"), "wrong operand count");
    assert_eq!(error(b"/x 1 m"), "wrong operand type");
    assert_eq!(error(b"1.0 J"), "wrong operand type");
    assert_eq!(error(b"EI"), "inline image delimiter outside image");
    assert_eq!(error(b"1 EI"), "wrong operand count");
    assert_eq!(error(b"(a) 1 Tf"), "wrong operand type");
    assert_eq!(error(b"1 /x ( ) \""), "wrong operand type");
}

#[test]
fn colour_operands_allow_a_final_pattern_name_only_with_scn() {
    let read = with_op(b"0.5 /P0 scn", |op| match op {
        Op::FillColorN(operands) => Some((operands.components.to_vec(), operands.pattern.clone())),
        _ => None,
    });
    assert_eq!(read.unwrap(), Some((vec![0.5], Some(Name::new(b"P0")))));
    assert_eq!(error(b"0.5 /P0 sc"), "wrong colour operand type");
    assert_eq!(error(b"/P0 0.5 scn"), "wrong colour operand type");
    let empty = with_op(b"SC", |op| matches!(op, Op::StrokeColor(c) if c.is_empty()));
    assert!(empty.unwrap());
    let long = format!("{}scn", "0.25 ".repeat(32));
    let count = with_op(long.as_bytes(), |op| match op {
        Op::FillColorN(operands) => operands.len(),
        _ => 0,
    });
    assert_eq!(count.unwrap(), 32);
}

#[test]
fn dash_arrays_and_text_arrays_are_validated() {
    let dash = with_op(b"[3 1.5] 2 d", |op| match op {
        Op::Dash(dash) => Some(dash.clone()),
        _ => None,
    });
    assert_eq!(
        dash.unwrap(),
        Some(Dash {
            lengths: vec![3.0, 1.5],
            phase: 2.0
        })
    );
    assert_eq!(error(b"[0] 2 d"), "dash array is all zero");
    assert_eq!(error(b"[-1] 2 d"), "invalid dash length");
    assert_eq!(error(b"[(a)] 2 d"), "invalid dash length");
    assert_eq!(error(b"[/a] 2 d"), "invalid dash length");
    assert_eq!(error(b"[1] /p d"), "wrong operand type");
    let items = with_op(b"[(A) -120 <42 43> 0.5] TJ", |op| match op {
        Op::ShowTextAdjusted(array) => format!("{array:?}"),
        _ => String::new(),
    });
    assert_eq!(
        items.unwrap(),
        "[Show([65]), Adjust(-120.0), Show([66, 67]), Adjust(0.5)]"
    );
    assert_eq!(error(b"[/x] TJ"), "TJ array requires numbers or strings");
    assert_eq!(error(b"[(a) [1]] TJ"), "TJ array requires numbers or strings");
}

#[test]
fn arrays_the_object_parser_reads_give_the_same_operations() {
    // With no nesting budget, arrays are left to the object parser.
    let limits = Limits {
        depth: 0,
        ..Limits::default()
    };
    let content = Content::from_bytes(b"[] TJ [] 0 d".as_slice());
    let mut reader = Reader::new(&content, &limits);
    let mut tokens = Meter::new(100, "test");
    let first = reader.next(&mut tokens).unwrap().unwrap();
    assert!(matches!(first.op, Op::ShowTextAdjusted(array) if array.is_empty()));
    let second = reader.next(&mut tokens).unwrap().unwrap();
    assert!(matches!(second.op, Op::Dash(Dash { ref lengths, .. }) if lengths.is_empty()));
}

#[test]
fn painting_operators_know_their_rules() {
    assert_eq!(Painting::FillStrokeEvenOdd.fill(), Some(Rule::EvenOdd));
    assert_eq!(Painting::FillObsolete.fill(), Some(Rule::NonZero));
    assert!(Painting::CloseFillStroke.closes() && Painting::CloseFillStroke.strokes());
    assert!(!Painting::EndPath.strokes() && Painting::EndPath.fill().is_none());
    assert!(Op::NextLine.is_text() && !Op::BeginText.is_text());
}
