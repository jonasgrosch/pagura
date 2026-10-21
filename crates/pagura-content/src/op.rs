//! Typed content operations with validated operands (Annex A, Table A.1).

mod operands;

use crate::Operator;
use operands::{
    array, dash, integer, marked, name, name_number, none, numbers, spaced_string, string,
};
use pagura_core::{Dictionary, Error, Name, Result, Value};

/// A fill or clipping rule (8.5.3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rule {
    /// The nonzero winding number rule.
    NonZero,
    /// The even-odd rule.
    EvenOdd,
}

/// A path-painting operator (8.5.3, Table 59). The standard closes the set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Painting {
    /// `S`.
    Stroke,
    /// `s`: close and stroke.
    CloseStroke,
    /// `f`.
    Fill,
    /// `F`, the obsolete spelling of `f`.
    FillObsolete,
    /// `f*`.
    FillEvenOdd,
    /// `B`.
    FillStroke,
    /// `B*`.
    FillStrokeEvenOdd,
    /// `b`: close, fill and stroke.
    CloseFillStroke,
    /// `b*`: close, fill (even-odd) and stroke.
    CloseFillStrokeEvenOdd,
    /// `n`: end the path without painting.
    EndPath,
}

impl Painting {
    /// Whether the operator closes the current subpath first.
    pub fn closes(self) -> bool {
        matches!(
            self,
            Self::CloseStroke | Self::CloseFillStroke | Self::CloseFillStrokeEvenOdd
        )
    }

    /// The fill rule, when the operator fills.
    pub fn fill(self) -> Option<Rule> {
        match self {
            Self::Fill | Self::FillObsolete | Self::FillStroke | Self::CloseFillStroke => {
                Some(Rule::NonZero)
            }
            Self::FillEvenOdd | Self::FillStrokeEvenOdd | Self::CloseFillStrokeEvenOdd => {
                Some(Rule::EvenOdd)
            }
            Self::Stroke | Self::CloseStroke | Self::EndPath => None,
        }
    }

    /// Whether the operator strokes.
    pub fn strokes(self) -> bool {
        matches!(
            self,
            Self::Stroke
                | Self::CloseStroke
                | Self::FillStroke
                | Self::FillStrokeEvenOdd
                | Self::CloseFillStroke
                | Self::CloseFillStrokeEvenOdd
        )
    }
}

/// A line dash pattern (8.4.3.6) after validation: nonnegative lengths, not
/// all zero.
#[derive(Debug, Clone, PartialEq)]
pub struct Dash {
    /// Dash and gap lengths; empty for a solid line.
    pub lengths: Vec<f64>,
    /// The phase.
    pub phase: f64,
}

impl Dash {
    /// Validates a dash array and phase, as `d` and an ExtGState `D` entry
    /// give them.
    ///
    /// # Errors
    ///
    /// Returns `Syntax` for an entry that is not a nonnegative number and for
    /// a nonempty array of zeros.
    pub fn new(array: &[Value], phase: f64) -> Result<Self> {
        let lengths = array
            .iter()
            .map(|n| {
                n.as_number()
                    .filter(|&n| n >= 0.0)
                    .ok_or(Error::syntax("invalid dash length"))
            })
            .collect::<Result<Vec<_>>>()?;
        if !lengths.is_empty() && lengths.iter().all(|&n| n == 0.0) {
            return Err(Error::syntax("dash array is all zero"));
        }
        Ok(Self { lengths, phase })
    }
}

/// One element of a `TJ` array (9.4.3).
#[derive(Debug, Clone, PartialEq)]
pub enum TextItem {
    /// A string to show.
    Show(Vec<u8>),
    /// A position adjustment in thousandths of text space units.
    Adjust(f64),
}

/// The operands of `SC`, `sc`, `SCN` and `scn` (8.6.8, Table 73).
#[derive(Debug, Clone, PartialEq)]
pub struct ColorOperands {
    /// The numeric components.
    pub components: Vec<f64>,
    /// The pattern name that ends `SCN` and `scn` operands, if any.
    pub pattern: Option<Name>,
}

impl ColorOperands {
    /// The number of operands, the pattern name included.
    pub fn len(&self) -> usize {
        self.components.len() + usize::from(self.pattern.is_some())
    }

    /// Whether there are no operands.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The properties operand of `BDC` and `DP` (14.6.2): a resource name or an
/// inline dictionary.
#[derive(Debug, Clone, PartialEq)]
pub enum Properties {
    /// A name in the `Properties` resources.
    Name(Name),
    /// An inline dictionary.
    Inline(Dictionary),
}

/// One content operation with its operands parsed and validated as Annex A
/// requires. Inline images are [`Op::InlineImage`], whose data the reader
/// yields separately.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Op {
    /// `q`.
    Save,
    /// `Q`.
    Restore,
    /// `cm` with `[a b c d e f]`.
    Concatenate([f64; 6]),
    /// `w`.
    LineWidth(f64),
    /// `J`.
    LineCap(i64),
    /// `j`.
    LineJoin(i64),
    /// `M`.
    MiterLimit(f64),
    /// `d`.
    Dash(Dash),
    /// `ri`.
    RenderingIntent(Name),
    /// `i`.
    Flatness(f64),
    /// `gs`.
    ExtGState(Name),
    /// `m`.
    MoveTo([f64; 2]),
    /// `l`.
    LineTo([f64; 2]),
    /// `c`.
    CurveTo([f64; 6]),
    /// `v`: the current point is the first control point.
    CurveToInitial([f64; 4]),
    /// `y`: the end point is the second control point.
    CurveToFinal([f64; 4]),
    /// `h`.
    ClosePath,
    /// `re` with `x y width height`.
    Rectangle([f64; 4]),
    /// A path-painting operator.
    Paint(Painting),
    /// `W` or `W*`.
    Clip(Rule),
    /// `BT`.
    BeginText,
    /// `ET`.
    EndText,
    /// `Tc`.
    CharSpacing(f64),
    /// `Tw`.
    WordSpacing(f64),
    /// `Tz`, in percent.
    HorizontalScaling(f64),
    /// `TL`.
    Leading(f64),
    /// `Tf`.
    Font {
        /// The font resource.
        name: Name,
        /// The size.
        size: f64,
    },
    /// `Tr`.
    RenderMode(i64),
    /// `Ts`.
    Rise(f64),
    /// `Td`.
    MoveText([f64; 2]),
    /// `TD`.
    MoveTextLeading([f64; 2]),
    /// `Tm`.
    TextMatrix([f64; 6]),
    /// `T*`.
    NextLine,
    /// `Tj`.
    ShowText(Vec<u8>),
    /// `TJ`.
    ShowTextAdjusted(Vec<TextItem>),
    /// `'`.
    NextLineShowText(Vec<u8>),
    /// `"`.
    NextLineShowTextSpaced {
        /// Word spacing.
        word: f64,
        /// Character spacing.
        character: f64,
        /// The string.
        text: Vec<u8>,
    },
    /// `d0` with `wx wy`.
    GlyphWidth([f64; 2]),
    /// `d1` with `wx wy llx lly urx ury`.
    GlyphWidthBounds([f64; 6]),
    /// `CS`.
    StrokeColorSpace(Name),
    /// `cs`.
    FillColorSpace(Name),
    /// `SC`.
    StrokeColor(ColorOperands),
    /// `sc`.
    FillColor(ColorOperands),
    /// `SCN`.
    StrokeColorN(ColorOperands),
    /// `scn`.
    FillColorN(ColorOperands),
    /// `G`.
    StrokeGray(f64),
    /// `g`.
    FillGray(f64),
    /// `RG`.
    StrokeRgb([f64; 3]),
    /// `rg`.
    FillRgb([f64; 3]),
    /// `K`.
    StrokeCmyk([f64; 4]),
    /// `k`.
    FillCmyk([f64; 4]),
    /// `sh`.
    Shading(Name),
    /// `Do`.
    XObject(Name),
    /// `BMC`.
    BeginMarkedContent(Name),
    /// `BDC`.
    BeginMarkedContentProperties {
        /// The tag.
        tag: Name,
        /// The properties.
        properties: Properties,
    },
    /// `MP`.
    MarkedPoint(Name),
    /// `DP`.
    MarkedPointProperties {
        /// The tag.
        tag: Name,
        /// The properties.
        properties: Properties,
    },
    /// `EMC`.
    EndMarkedContent,
    /// `BX`.
    BeginCompatibility,
    /// `EX`.
    EndCompatibility,
    /// `BI … ID`: an inline image's dictionary, abbreviations as written.
    /// Its data follows and must be read with
    /// [`Reader::inline_image_data`](crate::Reader::inline_image_data).
    InlineImage(Dictionary),
}

impl Op {
    /// Validates `operands` for `operator` and builds the operation.
    ///
    /// # Errors
    ///
    /// Returns `Syntax` for a wrong operand count or type, a malformed dash
    /// array or `TJ` element, and for `BI`, `ID` and `EI`, which only the
    /// [`Reader`](crate::Reader) frames.
    pub fn parse(operator: Operator, operands: Vec<Value>) -> Result<Self> {
        use Operator as O;
        Ok(match operator {
            O::Save => none(&operands, Self::Save)?,
            O::Restore => none(&operands, Self::Restore)?,
            O::Concatenate => Self::Concatenate(numbers(&operands)?),
            O::LineWidth => Self::LineWidth(single(&operands)?),
            O::LineCap => Self::LineCap(integer(&operands)?),
            O::LineJoin => Self::LineJoin(integer(&operands)?),
            O::MiterLimit => Self::MiterLimit(single(&operands)?),
            O::Dash => Self::Dash(dash(&operands)?),
            O::RenderingIntent => Self::RenderingIntent(name(operands)?),
            O::Flatness => Self::Flatness(single(&operands)?),
            O::ExtGState => Self::ExtGState(name(operands)?),
            O::MoveTo => Self::MoveTo(numbers(&operands)?),
            O::LineTo => Self::LineTo(numbers(&operands)?),
            O::CurveTo => Self::CurveTo(numbers(&operands)?),
            O::CurveToInitial => Self::CurveToInitial(numbers(&operands)?),
            O::CurveToFinal => Self::CurveToFinal(numbers(&operands)?),
            O::ClosePath => none(&operands, Self::ClosePath)?,
            O::Rectangle => Self::Rectangle(numbers(&operands)?),
            O::Clip => none(&operands, Self::Clip(Rule::NonZero))?,
            O::ClipEvenOdd => none(&operands, Self::Clip(Rule::EvenOdd))?,
            O::StrokeColorSpace => Self::StrokeColorSpace(name(operands)?),
            O::FillColorSpace => Self::FillColorSpace(name(operands)?),
            O::StrokeColor => Self::StrokeColor(color(operands, Pattern::Forbidden)?),
            O::FillColor => Self::FillColor(color(operands, Pattern::Forbidden)?),
            O::StrokeColorN => Self::StrokeColorN(color(operands, Pattern::Allowed)?),
            O::FillColorN => Self::FillColorN(color(operands, Pattern::Allowed)?),
            O::StrokeGray => Self::StrokeGray(single(&operands)?),
            O::FillGray => Self::FillGray(single(&operands)?),
            O::StrokeRgb => Self::StrokeRgb(numbers(&operands)?),
            O::FillRgb => Self::FillRgb(numbers(&operands)?),
            O::StrokeCmyk => Self::StrokeCmyk(numbers(&operands)?),
            O::FillCmyk => Self::FillCmyk(numbers(&operands)?),
            O::Shading => Self::Shading(name(operands)?),
            O::XObject => Self::XObject(name(operands)?),
            O::BeginMarkedContent => Self::BeginMarkedContent(name(operands)?),
            O::BeginMarkedContentProperties => {
                let (tag, properties) = marked(operands)?;
                Self::BeginMarkedContentProperties { tag, properties }
            }
            O::MarkedPoint => Self::MarkedPoint(name(operands)?),
            O::MarkedPointProperties => {
                let (tag, properties) = marked(operands)?;
                Self::MarkedPointProperties { tag, properties }
            }
            O::EndMarkedContent => none(&operands, Self::EndMarkedContent)?,
            O::BeginCompatibility => none(&operands, Self::BeginCompatibility)?,
            O::EndCompatibility => none(&operands, Self::EndCompatibility)?,
            O::BeginInlineImage | O::InlineImageData | O::EndInlineImage => {
                none(&operands, ())?;
                return Err(Error::syntax("inline image delimiter outside image"));
            }
            operator => {
                return Self::paint(operator, &operands)
                    .transpose()
                    .unwrap_or_else(|| Self::text(operator, operands));
            }
        })
    }

    /// The path-painting operators.
    fn paint(operator: Operator, operands: &[Value]) -> Result<Option<Self>> {
        use Operator as O;
        let painting = match operator {
            O::Stroke => Painting::Stroke,
            O::CloseStroke => Painting::CloseStroke,
            O::Fill => Painting::Fill,
            O::FillObsolete => Painting::FillObsolete,
            O::FillEvenOdd => Painting::FillEvenOdd,
            O::FillStroke => Painting::FillStroke,
            O::FillStrokeEvenOdd => Painting::FillStrokeEvenOdd,
            O::CloseFillStroke => Painting::CloseFillStroke,
            O::CloseFillStrokeEvenOdd => Painting::CloseFillStrokeEvenOdd,
            O::EndPath => Painting::EndPath,
            _ => return Ok(None),
        };
        none(operands, Self::Paint(painting)).map(Some)
    }

    /// The text and Type 3 glyph operators.
    fn text(operator: Operator, operands: Vec<Value>) -> Result<Self> {
        use Operator as O;
        Ok(match operator {
            O::BeginText => none(&operands, Self::BeginText)?,
            O::EndText => none(&operands, Self::EndText)?,
            O::CharSpacing => Self::CharSpacing(single(&operands)?),
            O::WordSpacing => Self::WordSpacing(single(&operands)?),
            O::HorizontalScaling => Self::HorizontalScaling(single(&operands)?),
            O::Leading => Self::Leading(single(&operands)?),
            O::Font => {
                let (name, size) = name_number(operands)?;
                Self::Font { name, size }
            }
            O::RenderMode => Self::RenderMode(integer(&operands)?),
            O::Rise => Self::Rise(single(&operands)?),
            O::MoveText => Self::MoveText(numbers(&operands)?),
            O::MoveTextLeading => Self::MoveTextLeading(numbers(&operands)?),
            O::TextMatrix => Self::TextMatrix(numbers(&operands)?),
            O::NextLine => none(&operands, Self::NextLine)?,
            O::ShowText => Self::ShowText(string(operands)?),
            O::NextLineShowText => Self::NextLineShowText(string(operands)?),
            O::NextLineShowTextSpaced => {
                let (word, character, text) = spaced_string(operands)?;
                Self::NextLineShowTextSpaced {
                    word,
                    character,
                    text,
                }
            }
            O::ShowTextAdjusted => Self::ShowTextAdjusted(text_items(array(operands)?)?),
            O::GlyphWidth => Self::GlyphWidth(numbers(&operands)?),
            O::GlyphWidthBounds => Self::GlyphWidthBounds(numbers(&operands)?),
            _ => return Err(Error::syntax("unknown content operator")),
        })
    }

    /// The operator that spells this operation.
    pub fn operator(&self) -> Operator {
        use Operator as O;
        match self {
            Self::Save => O::Save,
            Self::Restore => O::Restore,
            Self::Concatenate(_) => O::Concatenate,
            Self::LineWidth(_) => O::LineWidth,
            Self::LineCap(_) => O::LineCap,
            Self::LineJoin(_) => O::LineJoin,
            Self::MiterLimit(_) => O::MiterLimit,
            Self::Dash(_) => O::Dash,
            Self::RenderingIntent(_) => O::RenderingIntent,
            Self::Flatness(_) => O::Flatness,
            Self::ExtGState(_) => O::ExtGState,
            Self::MoveTo(_) => O::MoveTo,
            Self::LineTo(_) => O::LineTo,
            Self::CurveTo(_) => O::CurveTo,
            Self::CurveToInitial(_) => O::CurveToInitial,
            Self::CurveToFinal(_) => O::CurveToFinal,
            Self::ClosePath => O::ClosePath,
            Self::Rectangle(_) => O::Rectangle,
            Self::Paint(painting) => painting_operator(*painting),
            Self::Clip(Rule::NonZero) => O::Clip,
            Self::Clip(Rule::EvenOdd) => O::ClipEvenOdd,
            Self::StrokeColorSpace(_) => O::StrokeColorSpace,
            Self::FillColorSpace(_) => O::FillColorSpace,
            Self::StrokeColor(_) => O::StrokeColor,
            Self::FillColor(_) => O::FillColor,
            Self::StrokeColorN(_) => O::StrokeColorN,
            Self::FillColorN(_) => O::FillColorN,
            Self::StrokeGray(_) => O::StrokeGray,
            Self::FillGray(_) => O::FillGray,
            Self::StrokeRgb(_) => O::StrokeRgb,
            Self::FillRgb(_) => O::FillRgb,
            Self::StrokeCmyk(_) => O::StrokeCmyk,
            Self::FillCmyk(_) => O::FillCmyk,
            Self::Shading(_) => O::Shading,
            Self::XObject(_) => O::XObject,
            Self::BeginMarkedContent(_) => O::BeginMarkedContent,
            Self::BeginMarkedContentProperties { .. } => O::BeginMarkedContentProperties,
            Self::MarkedPoint(_) => O::MarkedPoint,
            Self::MarkedPointProperties { .. } => O::MarkedPointProperties,
            Self::EndMarkedContent => O::EndMarkedContent,
            Self::BeginCompatibility => O::BeginCompatibility,
            Self::EndCompatibility => O::EndCompatibility,
            Self::InlineImage(_) => O::BeginInlineImage,
            text => text_operator(text),
        }
    }

    /// Whether the operation belongs to text state, positioning or showing
    /// (9.3, 9.4), the operators that continue a text run.
    pub fn is_text(&self) -> bool {
        matches!(
            self,
            Self::ShowText(_)
                | Self::ShowTextAdjusted(_)
                | Self::NextLineShowText(_)
                | Self::NextLineShowTextSpaced { .. }
                | Self::CharSpacing(_)
                | Self::WordSpacing(_)
                | Self::HorizontalScaling(_)
                | Self::Leading(_)
                | Self::Rise(_)
                | Self::Font { .. }
                | Self::TextMatrix(_)
                | Self::MoveText(_)
                | Self::MoveTextLeading(_)
                | Self::NextLine
        )
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

fn text_operator(op: &Op) -> Operator {
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

/// One numeric operand.
fn single(operands: &[Value]) -> Result<f64> {
    numbers::<1>(operands).map(|[n]| n)
}

/// Whether `SCN`/`scn` may end with a pattern name.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pattern {
    Allowed,
    Forbidden,
}

fn color(operands: Vec<Value>, pattern: Pattern) -> Result<ColorOperands> {
    let count = operands.len();
    let mut components = Vec::with_capacity(count);
    let mut name = None;
    for (index, value) in operands.into_iter().enumerate() {
        match value {
            Value::Name(n) if pattern == Pattern::Allowed && index + 1 == count => name = Some(n),
            value => components.push(
                value
                    .as_number()
                    .ok_or(Error::syntax("wrong colour operand type"))?,
            ),
        }
    }
    Ok(ColorOperands {
        components,
        pattern: name,
    })
}

fn text_items(values: Vec<Value>) -> Result<Vec<TextItem>> {
    values
        .into_iter()
        .map(|value| match value {
            Value::String(bytes) => Ok(TextItem::Show(bytes)),
            value => value
                .as_number()
                .map(TextItem::Adjust)
                .ok_or(Error::syntax("TJ array requires numbers or strings")),
        })
        .collect()
}

#[cfg(test)]
mod tests;
