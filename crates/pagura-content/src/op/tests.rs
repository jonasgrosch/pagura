use super::*;
use crate::{Content, Reader};
use pagura_core::{Limits, Meter};

/// Reads the one operation of `content` and hands it to `check`.
fn with_op<T>(content: &[u8], check: impl FnOnce(&Op<'_>) -> T) -> Result<T> {
    let content = Content::from_bytes(content);
    let mut reader = Reader::new(&content, &Limits::default());
    let mut tokens = Meter::new(1_000, "test budget exceeded");
    let operation = reader
        .next(&mut tokens)?
        .ok_or(Error::syntax("no operation"))?;
    Ok(check(&operation.op))
}

/// The operator of the one operation of `content`.
fn first_operator(content: &[u8]) -> Result<Operator> {
    let content = Content::from_bytes(content);
    let mut reader = Reader::new(&content, &Limits::default());
    let mut tokens = Meter::new(1_000, "test budget exceeded");
    let operation = reader
        .next(&mut tokens)?
        .ok_or(Error::syntax("no operation"))?;
    Ok(operator(&operation.op))
}

fn error(content: &[u8]) -> &'static str {
    with_op(content, |_| ()).unwrap_err().message
}

#[test]
fn every_operator_round_trips_through_its_operation() {
    for spelled in crate::operator::tests::SPELLINGS {
        let operator = Operator::from_bytes(spelled.as_bytes()).unwrap();
        let operands = match spelled.as_bytes() {
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
        let content = format!("{operands} {spelled} ");
        match first_operator(content.as_bytes()) {
            Ok(read) => assert_eq!(read, operator, "{operator:?}"),
            Err(error) => assert!(
                matches!(spelled, "ID" | "EI" | "BI" | "EX"),
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
    assert_eq!(
        error(b"[(a) [1]] TJ"),
        "TJ array requires numbers or strings"
    );
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
}

/// The operator that spells `op`.
pub(crate) fn operator(op: &Op<'_>) -> Operator {
    use Operator as O;
    match op {
        Op::Save => O::Save,
        Op::Restore => O::Restore,
        Op::Concatenate(_) => O::Concatenate,
        Op::LineWidth(_) => O::LineWidth,
        Op::LineCap(_) => O::LineCap,
        Op::LineJoin(_) => O::LineJoin,
        Op::MiterLimit(_) => O::MiterLimit,
        Op::Dash(_) => O::Dash,
        Op::RenderingIntent(_) => O::RenderingIntent,
        Op::Flatness(_) => O::Flatness,
        Op::ExtGState(_) => O::ExtGState,
        Op::MoveTo(_) => O::MoveTo,
        Op::LineTo(_) => O::LineTo,
        Op::CurveTo(_) => O::CurveTo,
        Op::CurveToInitial(_) => O::CurveToInitial,
        Op::CurveToFinal(_) => O::CurveToFinal,
        Op::ClosePath => O::ClosePath,
        Op::Rectangle(_) => O::Rectangle,
        Op::Paint(painting) => painting_operator(*painting),
        Op::Clip(Rule::NonZero) => O::Clip,
        Op::Clip(Rule::EvenOdd) => O::ClipEvenOdd,
        Op::StrokeColorSpace(_) => O::StrokeColorSpace,
        Op::FillColorSpace(_) => O::FillColorSpace,
        Op::StrokeColor(_) => O::StrokeColor,
        Op::FillColor(_) => O::FillColor,
        Op::StrokeColorN(_) => O::StrokeColorN,
        Op::FillColorN(_) => O::FillColorN,
        Op::StrokeGray(_) => O::StrokeGray,
        Op::FillGray(_) => O::FillGray,
        Op::StrokeRgb(_) => O::StrokeRgb,
        Op::FillRgb(_) => O::FillRgb,
        Op::StrokeCmyk(_) => O::StrokeCmyk,
        Op::FillCmyk(_) => O::FillCmyk,
        Op::Shading(_) => O::Shading,
        Op::XObject(_) => O::XObject,
        Op::BeginMarkedContent(_) => O::BeginMarkedContent,
        Op::BeginMarkedContentProperties { .. } => O::BeginMarkedContentProperties,
        Op::MarkedPoint(_) => O::MarkedPoint,
        Op::MarkedPointProperties { .. } => O::MarkedPointProperties,
        Op::EndMarkedContent => O::EndMarkedContent,
        Op::BeginCompatibility => O::BeginCompatibility,
        Op::EndCompatibility => O::EndCompatibility,
        Op::InlineImage(_) => O::BeginInlineImage,
        text => text_operator(text),
    }
}

fn painting_operator(painting: Painting) -> Operator {
    use Operator as O;
    match painting {
        Painting::Stroke => O::Stroke,
        Painting::CloseStroke => O::CloseStroke,
        Painting::Fill => O::Fill,
        Painting::FillObsolete => O::FillObsolete,
        Painting::FillEvenOdd => O::FillEvenOdd,
        Painting::FillStroke => O::FillStroke,
        Painting::FillStrokeEvenOdd => O::FillStrokeEvenOdd,
        Painting::CloseFillStroke => O::CloseFillStroke,
        Painting::CloseFillStrokeEvenOdd => O::CloseFillStrokeEvenOdd,
        Painting::EndPath => O::EndPath,
    }
}

fn text_operator(op: &Op<'_>) -> Operator {
    use Operator as O;
    match op {
        Op::BeginText => O::BeginText,
        Op::EndText => O::EndText,
        Op::CharSpacing(_) => O::CharSpacing,
        Op::WordSpacing(_) => O::WordSpacing,
        Op::HorizontalScaling(_) => O::HorizontalScaling,
        Op::Leading(_) => O::Leading,
        Op::Font { .. } => O::Font,
        Op::RenderMode(_) => O::RenderMode,
        Op::Rise(_) => O::Rise,
        Op::MoveText(_) => O::MoveText,
        Op::MoveTextLeading(_) => O::MoveTextLeading,
        Op::TextMatrix(_) => O::TextMatrix,
        Op::NextLine => O::NextLine,
        Op::ShowText(_) => O::ShowText,
        Op::ShowTextAdjusted(_) => O::ShowTextAdjusted,
        Op::NextLineShowText(_) => O::NextLineShowText,
        Op::NextLineShowTextSpaced { .. } => O::NextLineShowTextSpaced,
        Op::GlyphWidth(_) => O::GlyphWidth,
        _ => O::GlyphWidthBounds,
    }
}
